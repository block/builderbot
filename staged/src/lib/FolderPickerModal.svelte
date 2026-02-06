<!--
  FolderPickerModal.svelte - Quick folder picker for opening repositories

  Optimized for fast keyboard-driven folder opening:
  - Starts at home directory
  - Type to search folders recursively (fuzzy match)
  - Recent repos shown for quick access
  - Arrow keys to navigate, Enter to open/select
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Folder, X, ChevronRight, Home, Clock, Search, Loader2, GitBranch } from 'lucide-svelte';
  import {
    listDirectory,
    getHomeDir,
    searchDirectories,
    type DirEntry,
    type RecentRepo,
  } from './services/files';
  import type { RepoEntry } from './stores/repoState.svelte';

  interface Props {
    /** List of recent repositories */
    recentRepos: RepoEntry[];
    /** List of suggested repositories (pre-loaded from Spotlight) */
    suggestedRepos?: RecentRepo[];
    /** Called when a folder is selected */
    onSelect: (path: string) => void;
    /** Called when modal is closed */
    onClose: () => void;
    /** Current repo path (to show as disabled/current) */
    currentPath?: string | null;
  }

  let {
    recentRepos,
    suggestedRepos: suggestedReposProp = [],
    onSelect,
    onClose,
    currentPath = null,
  }: Props = $props();

  // State
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

  // Are we in search mode (user has typed something)?
  let isSearching = $derived(query.length > 0);

  // Filter suggested repos to exclude ones already in recentRepos
  let suggestedRepos = $derived.by(() => {
    const recentPaths = new Set(recentRepos.map((r) => r.path));
    return suggestedReposProp.filter((r) => !recentPaths.has(r.path));
  });

  // Initialize on mount (runs once)
  onMount(() => {
    getHomeDir().then((dir) => {
      homeDir = dir;
      currentDir = dir;
    });
  });

  // Focus input on mount
  $effect(() => {
    if (inputEl) {
      inputEl.focus();
    }
  });

  // Load directory when currentDir changes (only when not searching)
  $effect(() => {
    if (currentDir && !isSearching) {
      loadDirectory(currentDir);
    }
  });

  // Debounced search when query changes
  $effect(() => {
    if (searchTimeout) {
      clearTimeout(searchTimeout);
    }

    // Need at least 2 chars to search (avoids expensive short queries)
    if (!query || query.length < 2) {
      searchResults = [];
      searching = false;
      return;
    }

    searching = true;

    searchTimeout = setTimeout(async () => {
      try {
        // Search from current directory - use deeper search when at home
        const depth = currentDir === homeDir ? 4 : 3;
        const results = await searchDirectories(currentDir, query, depth, 20);
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

  // Reset selection when results change
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
      // Only show directories
      entries = allEntries.filter((e) => e.isDir);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      entries = [];
    } finally {
      loading = false;
    }
  }

  // Filter recents by query
  let filteredRecents = $derived.by(() => {
    if (!query) return recentRepos.slice(0, 5);
    const q = query.toLowerCase();
    return recentRepos
      .filter((r) => r.name.toLowerCase().includes(q) || r.path.toLowerCase().includes(q))
      .slice(0, 5);
  });

  // Filter suggested repos by query
  let filteredSuggested = $derived.by(() => {
    if (!query) return suggestedRepos.slice(0, 5);
    const q = query.toLowerCase();
    return suggestedRepos
      .filter((r) => r.name.toLowerCase().includes(q) || r.path.toLowerCase().includes(q))
      .slice(0, 5);
  });

  // Combined list for selection
  let allItems = $derived.by(() => {
    if (isSearching) {
      // When searching: suggested first, then recents, then search results
      return [
        ...filteredSuggested.map((r) => ({ type: 'suggested' as const, ...r })),
        ...filteredRecents.map((r) => ({ type: 'recent' as const, ...r })),
        ...searchResults.map((e) => ({ type: 'search' as const, ...e })),
      ];
    } else {
      // When browsing: suggested (if at home), recents, then directory entries
      const showSpecial = currentDir === homeDir;
      return [
        ...(showSpecial
          ? filteredSuggested.map((r) => ({ type: 'suggested' as const, ...r }))
          : []),
        ...(showSpecial ? filteredRecents.map((r) => ({ type: 'recent' as const, ...r })) : []),
        ...entries.map((e) => ({ type: 'entry' as const, ...e })),
      ];
    }
  });

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
        // Clear search first
        query = '';
        event.preventDefault();
      } else {
        onClose();
        event.preventDefault();
      }
    } else if (event.key === 'Enter') {
      const item = allItems[selectedIndex];
      if (item) {
        // Recent repos and search results are always repos - select them
        // For directory entries, only select if it's a repo, otherwise drill in
        if (item.type === 'recent' || item.type === 'suggested' || item.type === 'search') {
          if (item.path !== currentPath) {
            onSelect(item.path);
          }
        } else if (item.type === 'entry') {
          if (item.isRepo) {
            onSelect(item.path);
          } else {
            // Not a repo - drill into it instead
            navigateTo(item.path);
          }
        }
      } else if (!isSearching && entries.length === 0) {
        // No entries - select current folder
        selectCurrent();
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
      // Arrow right or Tab drills into the folder
      const item = allItems[selectedIndex];
      if (item && item.type !== 'recent' && item.type !== 'suggested') {
        event.preventDefault();
        navigateTo(item.path);
      } else if (event.key === 'Tab') {
        event.preventDefault(); // Prevent tab from leaving modal
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

  /**
   * Highlight matching substring in text.
   */
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

  /**
   * Format path for display - collapse home directory to ~
   */
  function formatPath(path: string): string {
    if (homeDir && path.startsWith(homeDir)) {
      return '~' + path.slice(homeDir.length);
    }
    return path;
  }

  /**
   * Get breadcrumb segments from a path.
   */
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

  // Track index where recents start (after suggested)
  let firstRecentsIndex = $derived(
    isSearching ? filteredSuggested.length : currentDir === homeDir ? filteredSuggested.length : 0
  );

  // Track index where entries/search results start (after suggested and recents)
  let firstNonRecentIndex = $derived(
    isSearching
      ? filteredSuggested.length + filteredRecents.length
      : currentDir === homeDir
        ? filteredSuggested.length + filteredRecents.length
        : 0
  );
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
    <!-- Header with breadcrumbs -->
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

    <!-- Results -->
    <div class="results">
      {#if loading && !isSearching}
        <div class="empty-state">Loading...</div>
      {:else if error && !isSearching}
        <div class="empty-state error">{error}</div>
      {:else}
        <!-- Recent repos section -->
        <!-- Suggested repos section (recently active repos) - shown first -->
        {#if filteredSuggested.length > 0 && (isSearching || currentDir === homeDir)}
          <div class="section-header">
            <GitBranch size={12} />
            <span>Suggested</span>
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

        <!-- Recent repos section -->
        {#if filteredRecents.length > 0 && (isSearching || currentDir === homeDir)}
          <div class="section-header">
            <Clock size={12} />
            <span>Recent</span>
          </div>
          {#each filteredRecents as repo, i (repo.path)}
            {@const isCurrent = repo.path === currentPath}
            {@const isSelected = i + firstRecentsIndex === selectedIndex}
            <button
              class="result recent-result"
              class:selected={isSelected}
              class:current={isCurrent}
              onclick={() => !isCurrent && onSelect(repo.path)}
              disabled={isCurrent}
              onmouseenter={() => (selectedIndex = i + firstRecentsIndex)}
            >
              <Folder size={16} />
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

        <!-- Search results or folder entries -->
        {#if isSearching}
          {#if searchResults.length > 0}
            <div class="section-header">
              <Search size={12} />
              <span>Folders</span>
            </div>
            {#each searchResults as entry, i (entry.path)}
              {@const isSelected = i + firstNonRecentIndex === selectedIndex}
              <button
                class="result"
                class:selected={isSelected}
                onclick={() => onSelect(entry.path)}
                onmouseenter={() => (selectedIndex = i + firstNonRecentIndex)}
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
          {:else if !searching && filteredRecents.length === 0}
            <div class="empty-state">No matching folders</div>
          {/if}
        {:else if entries.length > 0}
          {#if currentDir === homeDir && filteredRecents.length > 0}
            <div class="section-header">
              <Folder size={12} />
              <span>Folders</span>
            </div>
          {/if}
          {#each entries as entry, i (entry.path)}
            {@const isSelected = i + firstNonRecentIndex === selectedIndex}
            <button
              class="result"
              class:selected={isSelected}
              class:is-repo={entry.isRepo}
              onclick={() => (entry.isRepo ? onSelect(entry.path) : navigateTo(entry.path))}
              onmouseenter={() => (selectedIndex = i + firstNonRecentIndex)}
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
        {:else if !loading && filteredRecents.length === 0}
          <div class="empty-state">Empty directory</div>
        {/if}
      {/if}
    </div>

    <!-- Footer -->
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

  .breadcrumb.home {
    padding: 4px 6px;
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
    background: var(--bg-secondary);
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
    background: var(--bg-secondary);
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
