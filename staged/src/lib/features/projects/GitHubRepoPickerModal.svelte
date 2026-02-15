<!--
  GitHubRepoPickerModal.svelte - Pick a GitHub repository

  Simplified UX:
  - Shows recently pushed repos on mount (across all orgs)
  - Type to search (debounced)
  - Typing "owner/repo" does a direct fetch
  - Pasting a GitHub URL selects immediately
  - No org dropdown needed
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { X, Search, Lock, Globe } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../commands';
  import type { GitHubRepo } from '../../types';

  interface Props {
    onSelect: (nameWithOwner: string) => void;
    onClose: () => void;
  }

  let { onSelect, onClose }: Props = $props();

  let repos = $state<GitHubRepo[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let query = $state('');
  let selectedIndex = $state(0);
  let searchInputEl: HTMLInputElement | null = $state(null);
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let isSearching = $state(false);

  // Track if we've done a direct fetch for the current query
  let directFetchRepo = $state<GitHubRepo | null>(null);

  /**
   * Parse a GitHub URL into owner/repo format.
   * Handles: https://github.com/owner/repo, github.com/owner/repo, etc.
   */
  function parseGitHubUrl(input: string): string | null {
    const trimmed = input.trim();
    const match = trimmed.match(
      /^(?:https?:\/\/)?github\.com\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)(?:\/.*|\.git)?$/
    );
    return match ? match[1] : null;
  }

  /**
   * Check if input looks like owner/repo format.
   */
  function isOwnerRepoFormat(input: string): boolean {
    const trimmed = input.trim();
    return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(trimmed);
  }

  /**
   * Build the display list: direct fetch result (if any) + filtered repos.
   */
  let displayRepos = $derived.by(() => {
    const result: GitHubRepo[] = [];

    // If we have a direct fetch result, show it first
    if (directFetchRepo) {
      result.push(directFetchRepo);
    }

    // Filter repos by query (client-side)
    const q = query.toLowerCase().trim();
    const filtered = q
      ? repos.filter(
          (r) =>
            r.nameWithOwner.toLowerCase().includes(q) ||
            (r.description && r.description.toLowerCase().includes(q))
        )
      : repos;

    // Add filtered repos, avoiding duplicates with direct fetch
    for (const r of filtered) {
      if (!directFetchRepo || r.nameWithOwner !== directFetchRepo.nameWithOwner) {
        result.push(r);
      }
    }

    return result;
  });

  onMount(async () => {
    try {
      // Load recently pushed repos (across all orgs)
      repos = await commands.listUserRepos(30);
    } catch (e) {
      error = typeof e === 'string' ? e : String(e);
    } finally {
      loading = false;
      searchInputEl?.focus();
    }
  });

  onDestroy(() => {
    if (searchTimer) clearTimeout(searchTimer);
  });

  async function handleInput() {
    const trimmed = query.trim();

    // Check if input is a GitHub URL — select immediately
    const parsed = parseGitHubUrl(trimmed);
    if (parsed) {
      onSelect(parsed);
      return;
    }

    // Clear previous direct fetch
    directFetchRepo = null;

    // Debounce search
    if (searchTimer) clearTimeout(searchTimer);

    if (!trimmed) {
      selectedIndex = 0;
      return;
    }

    searchTimer = setTimeout(async () => {
      // If it looks like owner/repo, try direct fetch first
      if (isOwnerRepoFormat(trimmed)) {
        const [owner, repo] = trimmed.split('/');
        isSearching = true;
        try {
          const result = await commands.getGithubRepo(owner, repo);
          if (result) {
            directFetchRepo = result;
            selectedIndex = 0;
          }
        } catch {
          // Direct fetch failed, fall through to search
        }
        isSearching = false;
      }

      // Also run a search to find partial matches
      if (trimmed.length >= 2) {
        isSearching = true;
        try {
          const results = await commands.searchGithubRepos(trimmed);
          // Merge search results with existing, deduplicating
          const existing = new Set(repos.map((r) => r.nameWithOwner));
          for (const r of results) {
            if (!existing.has(r.nameWithOwner)) {
              repos = [...repos, r];
            }
          }
        } catch {
          // Search failed — just use client-side filter
        }
        isSearching = false;
      }

      selectedIndex = 0;
    }, 300);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, displayRepos.length - 1);
      scrollSelectedIntoView();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
      scrollSelectedIntoView();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (displayRepos[selectedIndex]) {
        onSelect(displayRepos[selectedIndex].nameWithOwner);
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }

  function scrollSelectedIntoView() {
    const el = document.querySelector('.repo-item.selected');
    el?.scrollIntoView({ block: 'nearest' });
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={handleKeydown}
>
  <div class="modal">
    <div class="modal-header">
      <h2>Select Repository</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="search-bar">
      <Search size={14} class="search-icon" />
      <input
        bind:this={searchInputEl}
        bind:value={query}
        type="text"
        placeholder="Search repos or paste owner/repo..."
        autocomplete="off"
        autocorrect="off"
        spellcheck="false"
        oninput={handleInput}
      />
      {#if isSearching}
        <Spinner size={14} />
      {/if}
    </div>

    <div class="repo-list">
      {#if loading}
        <div class="loading-state">
          <Spinner size={20} />
          <span>Loading repositories...</span>
        </div>
      {:else if error}
        <div class="error-state">{error}</div>
      {:else if displayRepos.length === 0}
        <div class="empty-state">
          {#if query.trim()}
            {#if isOwnerRepoFormat(query.trim())}
              <span>No repository found for "{query.trim()}"</span>
            {:else}
              <span>No matching repositories</span>
            {/if}
          {:else}
            <span>No repositories found</span>
          {/if}
        </div>
      {:else}
        {#each displayRepos as repo, i (repo.nameWithOwner)}
          <button
            class="repo-item"
            class:selected={i === selectedIndex}
            onclick={() => onSelect(repo.nameWithOwner)}
            onmouseenter={() => (selectedIndex = i)}
          >
            <div class="repo-icon">
              {#if repo.isPrivate}
                <Lock size={14} />
              {:else}
                <Globe size={14} />
              {/if}
            </div>
            <div class="repo-info">
              <span class="repo-name">{repo.nameWithOwner}</span>
              {#if repo.description}
                <span class="repo-description">{repo.description}</span>
              {/if}
            </div>
          </button>
        {/each}
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    width: 520px;
    max-width: 90vw;
    max-height: 70vh;
    background-color: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    flex: 1;
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .close-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .search-bar input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-sm);
    color: var(--text-primary);
    padding: 6px 0;
  }

  .search-bar input::placeholder {
    color: var(--text-faint);
  }

  .repo-list {
    flex: 1;
    overflow-y: auto;
    padding: 4px 0;
    border-radius: 0 0 12px 12px;
  }

  .loading-state,
  .empty-state,
  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 32px 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .error-state {
    color: var(--ui-danger);
    text-align: center;
  }

  .repo-item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    width: 100%;
    padding: 8px 16px;
    background: none;
    border: none;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .repo-item:hover,
  .repo-item.selected {
    background-color: var(--bg-hover);
  }

  .repo-icon {
    padding-top: 2px;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .repo-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .repo-name {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .repo-description {
    font-size: var(--size-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
