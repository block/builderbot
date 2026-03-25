<!--
  FolderPickerModal — Spotlight-style folder picker for opening repositories.

  - Type to search for git repos recursively
  - Browse filesystem with breadcrumbs
  - Recently active repos via macOS Spotlight
  - Full keyboard navigation
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Folder, X, ChevronRight, Home, GitBranch, Search, Loader2 } from 'lucide-svelte';
  import * as commands from './commands';
  import type { DirEntry, RecentRepo } from './commands';

  interface Props {
    suggestedRepos?: RecentRepo[];
    loadingSuggestions?: boolean;
    onSelect: (path: string) => void;
    onClose: () => void;
    currentPath?: string | null;
  }

  let {
    suggestedRepos: suggestedReposProp = [],
    loadingSuggestions = false,
    onSelect,
    onClose,
    currentPath = null,
  }: Props = $props();

  let query = $state('');
  let currentDir = $state('');
  let entries = $state<DirEntry[]>([]);
  let searchResults = $state<DirEntry[]>([]);
  let loading = $state(false);
  let searching = $state(false);
  let error = $state<string | null>(null);
  let homeDir = $state('');
  let selectedIndex = $state(0);
  let inputEl: HTMLInputElement | null = $state(null);
  let searchTimeout: ReturnType<typeof setTimeout> | null = null;

  let isSearching = $derived(query.length > 0);

  let suggestedRepos = $derived(suggestedReposProp);

  onMount(() => {
    commands.getHomeDir().then((dir) => {
      homeDir = dir;
      currentDir = dir;
    });
  });

  $effect(() => {
    if (inputEl) {
      inputEl.focus();
    }
  });

  $effect(() => {
    if (currentDir && !isSearching) {
      loadDirectory(currentDir);
    }
  });

  $effect(() => {
    if (searchTimeout) {
      clearTimeout(searchTimeout);
    }

    if (!query || query.length < 2) {
      searchResults = [];
      searching = false;
      return;
    }

    searching = true;

    searchTimeout = setTimeout(async () => {
      try {
        const depth = currentDir === homeDir ? 4 : 3;
        const results = await commands.searchDirectories(currentDir, query, depth, 20);
        searchResults = results;
        selectedIndex = 0;
      } catch (e) {
        console.error('Search failed:', e);
        searchResults = [];
      } finally {
        searching = false;
      }
    }, 150);
  });

  $effect(() => {
    if (!isSearching) {
      const _ = entries;
      selectedIndex = 0;
    }
  });

  async function loadDirectory(path: string) {
    loading = true;
    error = null;
    try {
      const allEntries = await commands.listDirectory(path);
      entries = allEntries.filter((e) => e.isDir);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      entries = [];
    } finally {
      loading = false;
    }
  }

  let filteredSuggested = $derived.by(() => {
    if (!query) return suggestedRepos.slice(0, 5);
    const q = query.toLowerCase();
    return suggestedRepos
      .filter((r) => r.name.toLowerCase().includes(q) || r.path.toLowerCase().includes(q))
      .slice(0, 5);
  });

  let allItems = $derived.by(() => {
    if (isSearching) {
      return [
        ...filteredSuggested.map((r) => ({ type: 'suggested' as const, ...r })),
        ...searchResults.map((e) => ({ type: 'search' as const, ...e })),
      ];
    } else {
      const showSpecial = currentDir === homeDir;
      return [
        ...(showSpecial
          ? filteredSuggested.map((r) => ({ type: 'suggested' as const, ...r }))
          : []),
        ...entries.map((e) => ({ type: 'entry' as const, ...e })),
      ];
    }
  });

  let firstNonSuggestedIndex = $derived(
    isSearching ? filteredSuggested.length : currentDir === homeDir ? filteredSuggested.length : 0
  );

  function navigateTo(path: string) {
    query = '';
    currentDir = path;
  }

  function navigateUp() {
    if (currentDir === '/') return;
    const parent = currentDir.split('/').slice(0, -1).join('/') || '/';
    navigateTo(parent);
  }

  function navigateHome() {
    if (homeDir) {
      navigateTo(homeDir);
    }
  }

  function selectCurrent() {
    onSelect(currentDir);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      if (query) {
        query = '';
        event.preventDefault();
      } else {
        onClose();
        event.preventDefault();
      }
    } else if (event.key === 'Enter') {
      const item = allItems[selectedIndex];
      if (item) {
        if (item.type === 'suggested' || item.type === 'search') {
          if (item.path !== currentPath) {
            onSelect(item.path);
          }
        } else if (item.type === 'entry') {
          if (item.isRepo) {
            onSelect(item.path);
          } else {
            navigateTo(item.path);
          }
        }
      }
      event.preventDefault();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, allItems.length - 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
    } else if (event.key === 'ArrowLeft' && !isSearching) {
      event.preventDefault();
      navigateUp();
    } else if (event.key === 'ArrowRight' || event.key === 'Tab') {
      const item = allItems[selectedIndex];
      if (item && item.type !== 'suggested') {
        event.preventDefault();
        navigateTo(item.path);
      } else if (event.key === 'Tab') {
        event.preventDefault();
      }
    } else if (event.key === 'Backspace' && !query && !isSearching) {
      event.preventDefault();
      navigateUp();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }

  function highlightMatch(text: string, q: string): string {
    if (!q) return escapeHtml(text);
    const textLower = text.toLowerCase();
    const queryLower = q.toLowerCase();
    const idx = textLower.indexOf(queryLower);
    if (idx === -1) return escapeHtml(text);
    const before = escapeHtml(text.slice(0, idx));
    const match = escapeHtml(text.slice(idx, idx + q.length));
    const after = escapeHtml(text.slice(idx + q.length));
    return `${before}<mark>${match}</mark>${after}`;
  }

  function escapeHtml(str: string): string {
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function formatPath(path: string): string {
    if (homeDir && path.startsWith(homeDir)) {
      return '~' + path.slice(homeDir.length);
    }
    return path;
  }

  function getBreadcrumbs(path: string): { name: string; path: string }[] {
    const parts = path.split('/').filter(Boolean);
    const crumbs: { name: string; path: string }[] = [];
    let currentPath = '';
    for (const part of parts) {
      currentPath += '/' + part;
      crumbs.push({ name: part, path: currentPath });
    }
    return crumbs;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <div class="header">
      <div class="breadcrumbs">
        <button class="breadcrumb home" onclick={navigateHome} title="Home (~)">
          <Home size={14} />
        </button>
        {#each getBreadcrumbs(currentDir) as crumb, i}
          <ChevronRight size={12} class="separator" />
          <button
            class="breadcrumb"
            class:current={i === getBreadcrumbs(currentDir).length - 1}
            onclick={() => navigateTo(crumb.path)}
          >
            {crumb.name}
          </button>
        {/each}
      </div>
      <button class="close-btn" onclick={onClose} title="Close (Esc)">
        <X size={18} />
      </button>
    </div>

    <div class="search-container">
      <div class="search-icon">
        {#if searching}
          <Loader2 size={16} class="spinner" />
        {:else}
          <Search size={16} />
        {/if}
      </div>
      <input
        bind:this={inputEl}
        type="text"
        class="search-input"
        placeholder="Search folders..."
        bind:value={query}
        autocomplete="off"
        spellcheck="false"
      />
      {#if query}
        <button class="clear-btn" onclick={() => (query = '')} title="Clear">
          <X size={14} />
        </button>
      {/if}
    </div>

    <div class="results">
      {#if loading && !isSearching}
        <div class="empty-state">Loading...</div>
      {:else if error && !isSearching}
        <div class="empty-state error">{error}</div>
      {:else}
        {#if (filteredSuggested.length > 0 || loadingSuggestions) && (isSearching || currentDir === homeDir)}
          <div class="section-header">
            <GitBranch size={12} />
            <span>Suggested</span>
            {#if loadingSuggestions}
              <Loader2 size={12} class="spinner" />
            {/if}
          </div>
          {#each filteredSuggested as repo, i (repo.path)}
            {@const isCurrent = repo.path === currentPath}
            {@const isSelected = i === selectedIndex}
            <button
              class="result suggested-result"
              class:selected={isSelected}
              class:current={isCurrent}
              onclick={() => !isCurrent && onSelect(repo.path)}
              disabled={isCurrent}
              onmouseenter={() => (selectedIndex = i)}
            >
              <GitBranch size={16} class="suggested-icon" />
              <div class="result-info">
                <span class="result-name">{@html highlightMatch(repo.name, query)}</span>
                <span class="result-path">{@html highlightMatch(formatPath(repo.path), query)}</span
                >
              </div>
              {#if isCurrent}
                <span class="badge">Current</span>
              {:else}
                <ChevronRight size={14} class="action-hint" />
              {/if}
            </button>
          {/each}
        {/if}

        {#if isSearching}
          {#if searchResults.length > 0}
            <div class="section-header">
              <Search size={12} />
              <span>Folders</span>
            </div>
            {#each searchResults as entry, i (entry.path)}
              {@const isSelected = i + firstNonSuggestedIndex === selectedIndex}
              <button
                class="result"
                class:selected={isSelected}
                onclick={() => onSelect(entry.path)}
                onmouseenter={() => (selectedIndex = i + firstNonSuggestedIndex)}
              >
                <Folder size={16} />
                <div class="result-info">
                  <span class="result-name">{@html highlightMatch(entry.name, query)}</span>
                  <span class="result-path"
                    >{@html highlightMatch(formatPath(entry.path), query)}</span
                  >
                </div>
                <ChevronRight size={14} class="action-hint" />
              </button>
            {/each}
          {:else if !searching && filteredSuggested.length === 0}
            <div class="empty-state">No matching folders</div>
          {/if}
        {:else if entries.length > 0}
          {#if currentDir === homeDir && filteredSuggested.length > 0}
            <div class="section-header">
              <Folder size={12} />
              <span>Folders</span>
            </div>
          {/if}
          {#each entries as entry, i (entry.path)}
            {@const isSelected = i + firstNonSuggestedIndex === selectedIndex}
            <button
              class="result"
              class:selected={isSelected}
              class:is-repo={entry.isRepo}
              onclick={() => (entry.isRepo ? onSelect(entry.path) : navigateTo(entry.path))}
              onmouseenter={() => (selectedIndex = i + firstNonSuggestedIndex)}
            >
              {#if entry.isRepo}
                <GitBranch size={16} class="repo-icon" />
              {:else}
                <Folder size={16} />
              {/if}
              <span class="result-name">{entry.name}</span>
              <ChevronRight size={14} class="action-hint" />
            </button>
          {/each}
        {:else if !loading && filteredSuggested.length === 0}
          <div class="empty-state">Empty directory</div>
        {/if}
      {/if}
    </div>

    <div class="footer">
      <span class="hint">
        <kbd>↑↓</kbd> navigate
        <kbd>Enter</kbd> open
        <kbd>Tab</kbd> drill in
        <kbd>←</kbd> back
      </span>
      <button class="select-btn" onclick={selectCurrent}>
        Open {formatPath(currentDir)}
      </button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 520px;
    max-width: 90vw;
    max-height: 65vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px 10px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .breadcrumbs {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 2px;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .breadcrumbs::-webkit-scrollbar {
    display: none;
  }

  .breadcrumb {
    display: flex;
    align-items: center;
    padding: 4px 6px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    white-space: nowrap;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .breadcrumb:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .breadcrumb.current {
    color: var(--text-primary);
    font-weight: 500;
  }

  .breadcrumbs :global(.separator) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .close-btn,
  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover,
  .clear-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .search-container {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .search-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
  }

  :global(.spinner) {
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

  .search-input {
    flex: 1;
    padding: 4px 0;
    background: none;
    border: none;
    font-size: var(--size-base);
    color: var(--text-primary);
    outline: none;
    font-family: inherit;
  }

  .search-input::placeholder {
    color: var(--text-faint);
  }

  .results {
    flex: 1;
    overflow-y: auto;
    padding: 4px 0;
  }

  .section-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px 4px;
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .empty-state {
    padding: 24px 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .empty-state.error {
    color: var(--ui-danger);
  }

  .result {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 16px;
    background: none;
    border: none;
    text-align: left;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .result:hover,
  .result.selected {
    background-color: var(--bg-hover);
  }

  .result.current {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .result :global(svg) {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .result.is-repo :global(.repo-icon) {
    color: var(--text-accent);
  }

  .result.suggested-result :global(.suggested-icon) {
    color: var(--text-accent);
  }

  .result-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    overflow: hidden;
    min-width: 0;
  }

  .result-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .result-name :global(mark) {
    background: var(--bg-primary);
    color: var(--text-accent);
    border-radius: 2px;
    padding: 0 1px;
  }

  .result-path {
    font-size: var(--size-xs);
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .result-path :global(mark) {
    background: var(--bg-primary);
    color: var(--text-accent);
    border-radius: 2px;
    padding: 0 1px;
  }

  .badge {
    font-size: calc(var(--size-xs) - 1px);
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--bg-elevated);
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .result :global(.action-hint) {
    color: var(--text-faint);
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.1s;
  }

  .result:hover :global(.action-hint),
  .result.selected :global(.action-hint) {
    opacity: 1;
  }

  .footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 16px;
    border-top: 1px solid var(--border-subtle);
  }

  .hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  kbd {
    display: inline-block;
    padding: 2px 4px;
    margin: 0 2px;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 3px;
  }

  .select-btn {
    padding: 6px 12px;
    background: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.1s;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .select-btn:hover {
    background: var(--ui-accent-hover);
  }
</style>
