<!--
  GitHubRepoPicker.svelte - Embeddable GitHub repo picker (no modal chrome)

  Standalone search + list for picking a GitHub repository.
  Used inline inside NewProjectForm.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ArrowLeft, Search, Lock, Globe, Clock, Plus } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../api/commands';
  import type { GitHubRepo, RecentRepo } from '../../types';

  interface Props {
    onSelect: (nameWithOwner: string, subpath?: string) => void;
    onBack: () => void;
    excludeRepos?: Set<string>;
    showHeader?: boolean;
  }

  let { onSelect, onBack, excludeRepos = new Set(), showHeader = true }: Props = $props();

  let recentRepos = $state<RecentRepo[]>([]);
  let repos = $state<GitHubRepo[]>([]);
  let searchResults = $state<GitHubRepo[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let query = $state('');
  let searchInputEl: HTMLInputElement | null = $state(null);
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let isSearching = $state(false);
  let directFetchRepo = $state<GitHubRepo | null>(null);

  function parseGitHubUrl(input: string): string | null {
    const trimmed = input.trim();
    const match = trimmed.match(
      /^(?:https?:\/\/)?github\.com\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)(?:\/.*|\.git)?$/
    );
    return match ? match[1] : null;
  }

  function isOwnerRepoFormat(input: string): boolean {
    const trimmed = input.trim();
    return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(trimmed);
  }

  function parseQuery(input: string): { owner: string | null; term: string } {
    const trimmed = input.trim();
    const slashIndex = trimmed.indexOf('/');
    if (slashIndex > 0) {
      return {
        owner: trimmed.slice(0, slashIndex),
        term: trimmed.slice(slashIndex + 1),
      };
    }
    return { owner: null, term: trimmed };
  }

  let displayItems = $derived.by(() => {
    const seen = new Set<string>();
    const result: Array<{ type: 'recent' | 'repo'; data: RecentRepo | GitHubRepo }> = [];

    if (directFetchRepo && !excludeRepos.has(directFetchRepo.nameWithOwner)) {
      result.push({ type: 'repo', data: directFetchRepo });
      seen.add(directFetchRepo.nameWithOwner);
    }

    for (const r of searchResults) {
      if (!seen.has(r.nameWithOwner) && !excludeRepos.has(r.nameWithOwner)) {
        result.push({ type: 'repo', data: r });
        seen.add(r.nameWithOwner);
      }
    }

    const q = query.toLowerCase().trim();
    const filtered = q
      ? repos.filter(
          (r) =>
            r.nameWithOwner.toLowerCase().includes(q) ||
            (r.description && r.description.toLowerCase().includes(q))
        )
      : repos;

    for (const r of filtered) {
      if (!seen.has(r.nameWithOwner) && !excludeRepos.has(r.nameWithOwner)) {
        result.push({ type: 'repo', data: r });
        seen.add(r.nameWithOwner);
      }
    }

    return result;
  });

  let filteredRecentRepos = $derived.by(() => {
    const q = query.toLowerCase().trim();
    const base = recentRepos.filter((r) => !excludeRepos.has(r.githubRepo));
    return q ? base.filter((r) => r.githubRepo.toLowerCase().includes(q)) : base;
  });

  export function focusSearch() {
    searchInputEl?.focus();
  }

  onMount(async () => {
    searchInputEl?.focus();
    try {
      recentRepos = await commands.listRecentRepos(10);
      repos = await commands.listUserRepos(30);
    } catch (e) {
      error = typeof e === 'string' ? e : String(e);
    } finally {
      loading = false;
    }
  });

  onDestroy(() => {
    if (searchTimer) clearTimeout(searchTimer);
  });

  async function handleInput() {
    const trimmed = query.trim();

    const parsed = parseGitHubUrl(trimmed);
    if (parsed) {
      onSelect(parsed);
      return;
    }

    directFetchRepo = null;
    searchResults = [];

    if (searchTimer) clearTimeout(searchTimer);

    if (!trimmed) {
      isSearching = false;
      return;
    }

    isSearching = true;

    searchTimer = setTimeout(async () => {
      try {
        if (isOwnerRepoFormat(trimmed)) {
          const [owner, repo] = trimmed.split('/');
          try {
            const result = await commands.getGithubRepo(owner, repo);
            if (result) {
              directFetchRepo = result;
            }
          } catch {
            // Direct fetch failed, continue to search
          }
        }

        const { owner, term } = parseQuery(trimmed);

        if (term.length >= 1 || owner) {
          const searchQuery = term || (owner ? `org:${owner}` : '');
          if (searchQuery) {
            const results = await commands.searchGithubRepos(searchQuery, owner ?? undefined);
            searchResults = results;
          }
        }
      } catch {
        // Search failed — just use client-side filter
      }

      isSearching = false;
    }, 300);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onBack();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const items = Array.from(document.querySelectorAll<HTMLElement>('.repo-picker .repo-item'));
      if (items.length === 0) return;

      const current = document.activeElement as HTMLElement | null;
      const idx = current ? items.indexOf(current) : -1;
      const next =
        e.key === 'ArrowDown'
          ? items[Math.min(idx + 1, items.length - 1)]
          : items[Math.max(idx - 1, 0)];
      next?.focus();
      next?.scrollIntoView({ block: 'nearest' });
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="repo-picker" class:no-header={!showHeader} onkeydown={handleKeydown}>
  {#if showHeader}
    <div class="picker-header">
      <button class="back-button" onclick={onBack}>
        <ArrowLeft size={16} />
      </button>
      <h2>Select Repository</h2>
    </div>
  {/if}

  <div class="search-bar">
    <Search size={14} class="search-icon" />
    <input
      bind:this={searchInputEl}
      bind:value={query}
      type="text"
      placeholder="Search or paste a repository..."
      autocomplete="off"
      autocorrect="off"
      spellcheck="false"
      oninput={handleInput}
    />
  </div>

  <div class="repo-list">
    {#if filteredRecentRepos.length > 0}
      {#each filteredRecentRepos as recent}
        <button
          class="repo-item recent"
          tabindex="-1"
          onclick={() => onSelect(recent.githubRepo, recent.subpath ?? undefined)}
        >
          <div class="repo-icon recent-icon">
            <Clock size={14} />
          </div>
          <div class="repo-info">
            <span class="repo-name">
              {recent.githubRepo}{#if recent.subpath}<span class="repo-subpath"
                  >/{recent.subpath}</span
                >{/if}
            </span>
          </div>
          <div class="repo-action">
            <Plus size={14} />
          </div>
        </button>
      {/each}
    {/if}

    {#if loading}
      <div class="loading-state">
        <Spinner size={20} />
      </div>
    {:else if error}
      <div class="error-state">{error}</div>
    {:else if isSearching}
      <div class="loading-state">
        <Spinner size={20} />
      </div>
    {:else if displayItems.length === 0 && filteredRecentRepos.length === 0}
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
      {#each displayItems as item}
        {#if item.type === 'repo'}
          {@const repo = item.data as GitHubRepo}
          <button class="repo-item" tabindex="-1" onclick={() => onSelect(repo.nameWithOwner)}>
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
            <div class="repo-action">
              <Plus size={14} />
            </div>
          </button>
        {/if}
      {/each}
    {/if}
  </div>
</div>

<style>
  .repo-picker {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .repo-picker.no-header .search-bar {
    margin-top: 10px;
  }

  .picker-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
  }

  .picker-header h2 {
    flex: 1;
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .back-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .back-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 10px 4px;
    padding: 10px 14px;
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    background: var(--bg-primary);
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
    padding: 0;
  }

  .search-bar input::placeholder {
    color: var(--text-faint);
  }

  .repo-list {
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 10px 10px;
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
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 12px 14px;
    background: none;
    border: none;
    border-radius: 10px;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .repo-item:hover,
  .repo-item:focus {
    background-color: var(--bg-hover);
    outline: none;
  }

  .repo-icon {
    color: var(--text-muted);
    flex-shrink: 0;
    display: flex;
  }

  .recent-icon {
    color: var(--ui-accent);
  }

  .repo-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    text-align: left;
  }

  .repo-name {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-subpath {
    font-weight: 400;
    color: var(--text-muted);
  }

  .repo-description {
    font-size: var(--size-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-action {
    color: var(--text-faint);
    flex-shrink: 0;
    display: flex;
    transition: color 0.15s ease;
  }

  .repo-item:hover .repo-action,
  .repo-item:focus .repo-action {
    color: var(--text-muted);
  }
</style>
