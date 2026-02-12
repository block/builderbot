<!--
  FolderPickerModal.svelte - Quick folder picker for git repositories

  Keyboard-driven repo selection:
  - Starts at home directory
  - Type to search repos (Spotlight + recursive dir search combined)
  - Suggested repos (from Spotlight) shown for quick access
  - Arrow keys to navigate, Enter to open, Tab/→ to drill in
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Folder, X, ChevronRight, Home, Search, GitBranch } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import {
    listDirectory,
    getHomeDir,
    searchDirectories,
    findRecentRepos,
    type DirEntry,
    type RecentRepo,
  } from '../../shared/files';

  interface Props {
    onSelect: (path: string) => void;
    onClose: () => void;
  }

  let { onSelect, onClose }: Props = $props();

  // State
  let query = $state('');
  let currentDir = $state('');
  let entries = $state<DirEntry[]>([]);
  let searchResults = $state<DirEntry[]>([]);
  let spotlightRepos = $state<RecentRepo[]>([]);
  let spotlightLoading = $state(true);
  let loading = $state(false);
  let searching = $state(false);
  let error = $state<string | null>(null);
  let homeDir = $state('');
  let selectedIndex = $state(0);
  let inputEl: HTMLInputElement | null = $state(null);
  let listEl: HTMLDivElement | null = $state(null);
  let searchTimeout: ReturnType<typeof setTimeout> | null = null;

  let isSearching = $derived(query.length > 0);

  onMount(() => {
    getHomeDir().then((dir) => {
      homeDir = dir;
      currentDir = dir;
    });
    // Load Spotlight suggestions in background
    findRecentRepos(48, 15)
      .then((repos) => {
        spotlightRepos = repos;
      })
      .finally(() => {
        spotlightLoading = false;
      });
  });

  $effect(() => {
    inputEl?.focus();
  });

  // Load directory when currentDir changes (only when not searching)
  $effect(() => {
    if (currentDir && !isSearching) {
      loadDirectory(currentDir);
    }
  });

  // Debounced search
  $effect(() => {
    if (searchTimeout) clearTimeout(searchTimeout);

    if (!query || query.length < 2) {
      searchResults = [];
      searching = false;
      return;
    }

    searching = true;

    searchTimeout = setTimeout(async () => {
      try {
        const depth = currentDir === homeDir ? 4 : 3;
        const results = await searchDirectories(currentDir, query, depth, 20);
        searchResults = results;
        selectedIndex = 0;
      } catch {
        searchResults = [];
      } finally {
        searching = false;
      }
    }, 150);
  });

  // Reset selection on directory change
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
      const allEntries = await listDirectory(path);
      entries = allEntries.filter((e) => e.isDir);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      entries = [];
    } finally {
      loading = false;
    }
  }

  // Filter Spotlight repos by query
  let filteredSpotlight = $derived.by(() => {
    const repos = spotlightRepos;
    if (!query) return repos.slice(0, 8);
    const q = query.toLowerCase();
    return repos
      .filter((r) => r.name.toLowerCase().includes(q) || r.path.toLowerCase().includes(q))
      .slice(0, 8);
  });

  // Combined list for keyboard navigation
  let allItems = $derived.by(() => {
    if (isSearching) {
      return [
        ...filteredSpotlight.map((r) => ({ type: 'spotlight' as const, ...r })),
        ...searchResults.map((e) => ({ type: 'search' as const, ...e })),
      ];
    }
    const showSpotlight = currentDir === homeDir;
    return [
      ...(showSpotlight
        ? filteredSpotlight.map((r) => ({ type: 'spotlight' as const, ...r }))
        : []),
      ...entries.map((e) => ({ type: 'entry' as const, ...e })),
    ];
  });

  let spotlightCount = $derived(
    isSearching ? filteredSpotlight.length : currentDir === homeDir ? filteredSpotlight.length : 0
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
    if (homeDir) navigateTo(homeDir);
  }

  function scrollSelectedIntoView() {
    if (!listEl) return;
    const el = listEl.querySelector('.result.selected');
    el?.scrollIntoView({ block: 'nearest' });
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      if (query) {
        query = '';
      } else {
        onClose();
      }
      event.preventDefault();
    } else if (event.key === 'Enter') {
      const item = allItems[selectedIndex];
      if (item) {
        if (item.type === 'spotlight' || item.type === 'search') {
          onSelect(item.path);
        } else if (item.type === 'entry') {
          if (item.isRepo) {
            onSelect(item.path);
          } else {
            navigateTo(item.path);
          }
        }
      } else if (!isSearching && entries.length === 0) {
        onSelect(currentDir);
      }
      event.preventDefault();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, allItems.length - 1);
      requestAnimationFrame(scrollSelectedIntoView);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
      requestAnimationFrame(scrollSelectedIntoView);
    } else if (event.key === 'ArrowLeft' && !isSearching) {
      event.preventDefault();
      navigateUp();
    } else if (event.key === 'ArrowRight' || event.key === 'Tab') {
      const item = allItems[selectedIndex];
      if (item && item.type !== 'spotlight') {
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
    if (event.target === event.currentTarget) onClose();
  }

  function highlightMatch(text: string, q: string): string {
    if (!q) return escapeHtml(text);
    const idx = text.toLowerCase().indexOf(q.toLowerCase());
    if (idx === -1) return escapeHtml(text);
    const before = escapeHtml(text.slice(0, idx));
    const match = escapeHtml(text.slice(idx, idx + q.length));
    const after = escapeHtml(text.slice(idx + q.length));
    return `${before}<mark>${match}</mark>${after}`;
  }

  function escapeHtml(str: string): string {
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function formatPath(path: string): string {
    if (homeDir && path.startsWith(homeDir)) return '~' + path.slice(homeDir.length);
    return path;
  }

  function getBreadcrumbs(path: string): { name: string; path: string }[] {
    const parts = path.split('/').filter(Boolean);
    let current = '';
    return parts.map((part) => {
      current += '/' + part;
      return { name: part, path: current };
    });
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
    <!-- Breadcrumb header -->
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

    <!-- Search input -->
    <div class="search-container">
      <div class="search-icon">
        {#if searching}
          <Spinner size={16} />
        {:else}
          <Search size={16} />
        {/if}
      </div>
      <input
        bind:this={inputEl}
        type="text"
        class="search-input"
        placeholder="Search repositories..."
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

    <!-- Results -->
    <div class="results" bind:this={listEl}>
      {#if loading && !isSearching}
        <div class="empty-state">Loading...</div>
      {:else if error && !isSearching}
        <div class="empty-state error">{error}</div>
      {:else}
        <!-- Spotlight suggestions -->
        {#if (spotlightLoading || filteredSpotlight.length > 0) && (isSearching || currentDir === homeDir)}
          <div class="section-header">
            <GitBranch size={12} />
            <span>Suggested</span>
            {#if spotlightLoading}
              <Spinner size={12} />
            {/if}
          </div>
          {#if spotlightLoading && filteredSpotlight.length === 0}
            <div class="skeleton-row">
              <div class="skeleton-pill"></div>
              <div class="skeleton-line"></div>
            </div>
          {:else}
            {#each filteredSpotlight as repo, i (repo.path)}
              {@const isSelected = i === selectedIndex}
              <button
                class="result"
                class:selected={isSelected}
                onclick={() => onSelect(repo.path)}
                onmouseenter={() => (selectedIndex = i)}
              >
                <GitBranch size={16} class="repo-icon" />
                <div class="result-info">
                  <span class="result-name">{@html highlightMatch(repo.name, query)}</span>
                  <span class="result-path"
                    >{@html highlightMatch(formatPath(repo.path), query)}</span
                  >
                </div>
                <ChevronRight size={14} class="action-hint" />
              </button>
            {/each}
          {/if}
        {/if}

        <!-- Search results or directory listing -->
        {#if isSearching}
          {#if searchResults.length > 0}
            <div class="section-header">
              <Search size={12} />
              <span>Repositories</span>
            </div>
            {#each searchResults as entry, i (entry.path)}
              {@const isSelected = i + spotlightCount === selectedIndex}
              <button
                class="result"
                class:selected={isSelected}
                onclick={() => onSelect(entry.path)}
                onmouseenter={() => (selectedIndex = i + spotlightCount)}
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
          {:else if !searching && filteredSpotlight.length === 0}
            <div class="empty-state">No matching repositories</div>
          {/if}
        {:else if entries.length > 0}
          {#if currentDir === homeDir && (spotlightLoading || filteredSpotlight.length > 0)}
            <div class="section-header">
              <Folder size={12} />
              <span>Folders</span>
            </div>
          {/if}
          {#each entries as entry, i (entry.path)}
            {@const isSelected = i + spotlightCount === selectedIndex}
            <button
              class="result"
              class:selected={isSelected}
              class:is-repo={entry.isRepo}
              onclick={() => (entry.isRepo ? onSelect(entry.path) : navigateTo(entry.path))}
              onmouseenter={() => (selectedIndex = i + spotlightCount)}
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
        {:else if !loading && filteredSpotlight.length === 0}
          <div class="empty-state">Empty directory</div>
        {/if}
      {/if}
    </div>

    <!-- Footer with keyboard hints -->
    <div class="footer">
      <span class="hint">
        <kbd>↑↓</kbd> navigate
        <kbd>Enter</kbd> select
        <kbd>Tab</kbd> drill in
        <kbd>←</kbd> back
      </span>
      <button class="select-btn" onclick={() => onSelect(currentDir)}>
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
    color: var(--text-muted);
  }

  .search-input {
    flex: 1;
    padding: 4px 0;
    background: none;
    border: none;
    font-size: var(--size-base);
    color: var(--text-primary);
    outline: none;
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

  .skeleton-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
  }

  .skeleton-pill {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    background: var(--bg-hover);
    animation: pulse 1.5s ease-in-out infinite;
  }

  .skeleton-line {
    height: 10px;
    width: 120px;
    border-radius: 4px;
    background: var(--bg-hover);
    animation: pulse 1.5s ease-in-out infinite;
    animation-delay: 0.1s;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 0.4;
    }
    50% {
      opacity: 0.8;
    }
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
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .result:hover,
  .result.selected {
    background-color: var(--bg-hover);
  }

  .result :global(svg) {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .result.is-repo :global(.repo-icon),
  .result :global(.repo-icon) {
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
    font-family: 'SF Mono', 'Menlo', monospace;
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
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: calc(var(--size-xs) - 1px);
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: 3px;
  }

  .select-btn {
    padding: 6px 12px;
    background: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
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
