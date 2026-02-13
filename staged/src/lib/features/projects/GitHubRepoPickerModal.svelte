<!--
  GitHubRepoPickerModal.svelte - Pick a GitHub repository

  Keyboard-driven repo selection:
  - Lists authenticated user's repos on mount
  - Type to filter/search
  - Arrow keys to navigate, Enter to select
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { X, Search, Lock, Globe, ChevronDown, Check } from 'lucide-svelte';
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

  // Org selector state
  let orgs = $state<string[]>([]);
  let selectedOwner = $state<string | null>(null); // null = personal / @me
  let ownerDropdownOpen = $state(false);
  let ownerLabel = $derived(selectedOwner ?? 'My repos');

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

  let filteredRepos = $derived.by(() => {
    if (!query.trim()) return repos;
    const q = query.toLowerCase();
    return repos.filter(
      (r) =>
        r.nameWithOwner.toLowerCase().includes(q) ||
        (r.description && r.description.toLowerCase().includes(q))
    );
  });

  onMount(async () => {
    const [orgsResult, reposResult] = await Promise.allSettled([
      commands.listGithubOrgs(),
      commands.listGithubRepos(),
    ]);
    if (orgsResult.status === 'fulfilled') {
      orgs = orgsResult.value;
    }
    if (reposResult.status === 'fulfilled') {
      repos = reposResult.value;
    } else {
      error =
        typeof reposResult.reason === 'string' ? reposResult.reason : String(reposResult.reason);
    }
    loading = false;
    searchInputEl?.focus();
    document.addEventListener('click', handleOwnerClickOutside);
  });

  async function selectOwner(owner: string | null) {
    ownerDropdownOpen = false;
    if (owner === selectedOwner) return;
    selectedOwner = owner;
    query = '';
    selectedIndex = 0;
    loading = true;
    error = null;
    try {
      repos = await commands.listGithubRepos(owner ?? undefined);
    } catch (e) {
      error = typeof e === 'string' ? e : String(e);
      repos = [];
    } finally {
      loading = false;
    }
  }

  function handleOwnerClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (ownerDropdownOpen && !target.closest('.owner-dropdown')) {
      ownerDropdownOpen = false;
    }
  }

  onDestroy(() => {
    document.removeEventListener('click', handleOwnerClickOutside);
  });

  async function handleSearch() {
    // Check if input is a GitHub URL — select immediately
    const parsed = parseGitHubUrl(query);
    if (parsed) {
      onSelect(parsed);
      return;
    }

    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(async () => {
      if (query.trim().length >= 2) {
        try {
          const results = await commands.searchGithubRepos(
            query.trim(),
            selectedOwner ?? undefined
          );
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
      }
      selectedIndex = 0;
    }, 300);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, filteredRepos.length - 1);
      scrollSelectedIntoView();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
      scrollSelectedIntoView();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (filteredRepos[selectedIndex]) {
        onSelect(filteredRepos[selectedIndex].nameWithOwner);
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
        placeholder="Search or paste a GitHub URL..."
        autocomplete="off"
        autocorrect="off"
        spellcheck="false"
        oninput={handleSearch}
      />
      {#if orgs.length > 0}
        <div class="owner-dropdown">
          <button
            type="button"
            class="owner-btn"
            onclick={() => (ownerDropdownOpen = !ownerDropdownOpen)}
          >
            <span class="owner-label">{ownerLabel}</span>
            <ChevronDown size={14} />
          </button>
          {#if ownerDropdownOpen}
            <div class="owner-menu">
              <button
                type="button"
                class="owner-menu-item"
                class:selected={selectedOwner === null}
                onclick={() => selectOwner(null)}
              >
                <span>My repos</span>
                {#if selectedOwner === null}
                  <Check size={14} />
                {/if}
              </button>
              {#each orgs as org (org)}
                <button
                  type="button"
                  class="owner-menu-item"
                  class:selected={selectedOwner === org}
                  onclick={() => selectOwner(org)}
                >
                  <span>{org}</span>
                  {#if selectedOwner === org}
                    <Check size={14} />
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>
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
      {:else if filteredRepos.length === 0}
        <div class="empty-state">
          {query ? 'No matching repositories' : 'No repositories found'}
        </div>
      {:else}
        {#each filteredRepos as repo, i (repo.nameWithOwner)}
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
    overflow: hidden;
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

  .owner-dropdown {
    position: relative;
    flex-shrink: 0;
  }

  .owner-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .owner-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .owner-label {
    white-space: nowrap;
  }

  .owner-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
    z-index: 10;
    min-width: 160px;
    max-height: 200px;
    overflow-y: auto;
  }

  .owner-menu-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .owner-menu-item:hover {
    background-color: var(--bg-hover);
  }

  .owner-menu-item.selected {
    color: var(--ui-accent);
  }

  .owner-menu-item.selected :global(svg) {
    color: var(--ui-accent);
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
