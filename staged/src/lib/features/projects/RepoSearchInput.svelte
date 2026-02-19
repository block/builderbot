<!--
  RepoSearchInput.svelte - Inline repo search with dropdown results

  Self-contained text input that fetches recent/user repos on mount
  and shows an absolutely positioned dropdown when focused.
  Used in NewProjectForm for the repository field.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Search, Lock, Globe, Clock, Plus, Command } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../commands';
  import type { GitHubRepo, RecentRepo } from '../../types';

  interface Props {
    onSelect: (nameWithOwner: string, subpath?: string) => void;
    disabled?: boolean;
  }

  let { onSelect, disabled = false }: Props = $props();

  let recentRepos = $state<RecentRepo[]>([]);
  let repos = $state<GitHubRepo[]>([]);
  let searchResults = $state<GitHubRepo[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let query = $state('');
  let inputEl: HTMLInputElement | null = $state(null);
  let wrapperEl: HTMLDivElement | null = $state(null);
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let isSearching = $state(false);
  let directFetchRepo = $state<GitHubRepo | null>(null);
  let dropdownOpen = $state(false);
  let dropdownStyle = $state('');

  function updateDropdownPosition() {
    if (!inputEl) return;
    const rect = inputEl.closest('.search-input')!.getBoundingClientRect();
    const top = rect.bottom + 4;
    const maxH = Math.max(120, window.innerHeight - top - 12);
    dropdownStyle = `position:fixed;top:${top}px;left:${rect.left}px;width:${rect.width}px;max-height:${maxH}px`;
  }

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

    if (directFetchRepo) {
      result.push({ type: 'repo', data: directFetchRepo });
      seen.add(directFetchRepo.nameWithOwner);
    }

    for (const r of searchResults) {
      if (!seen.has(r.nameWithOwner)) {
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
      if (!seen.has(r.nameWithOwner)) {
        result.push({ type: 'repo', data: r });
        seen.add(r.nameWithOwner);
      }
    }

    return result;
  });

  let filteredRecentRepos = $derived.by(() => {
    const q = query.toLowerCase().trim();
    return q ? recentRepos.filter((r) => r.githubRepo.toLowerCase().includes(q)) : recentRepos;
  });

  onMount(async () => {
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

  function handleSelect(nameWithOwner: string, subpath?: string) {
    query = '';
    dropdownOpen = false;
    onSelect(nameWithOwner, subpath);
  }

  async function handleInput() {
    const trimmed = query.trim();

    const parsed = parseGitHubUrl(trimmed);
    if (parsed) {
      handleSelect(parsed);
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

  function handleFocus() {
    updateDropdownPosition();
    dropdownOpen = true;
  }

  function handleFocusOut(e: FocusEvent) {
    const related = e.relatedTarget as Node | null;
    if (wrapperEl && related && wrapperEl.contains(related)) return;
    dropdownOpen = false;
  }

  function handleClickOutside(e: MouseEvent) {
    if (wrapperEl && !wrapperEl.contains(e.target as Node)) {
      dropdownOpen = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!dropdownOpen) return;

    if (e.key === 'Escape') {
      e.preventDefault();
      dropdownOpen = false;
      inputEl?.focus();
    } else if (e.metaKey && e.key >= '1' && e.key <= '9') {
      const idx = parseInt(e.key) - 1;
      const items = Array.from(wrapperEl?.querySelectorAll<HTMLElement>('.repo-item') ?? []);
      if (idx < items.length) {
        e.preventDefault();
        items[idx].click();
      }
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const items = Array.from(wrapperEl?.querySelectorAll<HTMLElement>('.repo-item') ?? []);
      if (items.length === 0) return;

      const current = document.activeElement as HTMLElement | null;
      const idx = current ? items.indexOf(current) : -1;

      if (e.key === 'ArrowDown') {
        const next = items[Math.min(idx + 1, items.length - 1)];
        next?.focus();
        next?.scrollIntoView({ block: 'nearest' });
      } else {
        if (idx <= 0) {
          inputEl?.focus();
        } else {
          const prev = items[idx - 1];
          prev?.focus();
          prev?.scrollIntoView({ block: 'nearest' });
        }
      }
    }
  }
</script>

<svelte:window
  onclick={handleClickOutside}
  onresize={() => dropdownOpen && updateDropdownPosition()}
/>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="repo-search-wrapper"
  bind:this={wrapperEl}
  onkeydown={handleKeydown}
  onfocusout={handleFocusOut}
>
  <div class="search-input">
    <Search size={14} class="search-icon" />
    <input
      bind:this={inputEl}
      bind:value={query}
      type="text"
      placeholder="Search or paste a repository..."
      autocomplete="off"
      autocorrect="off"
      spellcheck="false"
      {disabled}
      oninput={handleInput}
      onfocus={handleFocus}
    />
  </div>

  {#if dropdownOpen}
    <div class="repo-dropdown" style={dropdownStyle}>
      {#if filteredRecentRepos.length > 0}
        {#each filteredRecentRepos as recent, i}
          <button
            class="repo-item recent"
            tabindex="-1"
            onclick={() => handleSelect(recent.githubRepo, recent.subpath ?? undefined)}
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
            {#if i < 3}
              <span class="keyboard-shortcut">
                <Command size={10} />
                {i + 1}
              </span>
            {/if}
            <div class="repo-action">
              <Plus size={14} />
            </div>
          </button>
        {/each}
      {/if}

      {#if loading}
        <div class="loading-state">
          <Spinner size={16} />
        </div>
      {:else if error}
        <div class="error-state">{error}</div>
      {:else if isSearching}
        <div class="loading-state">
          <Spinner size={16} />
          <span>Searching...</span>
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
            <button
              class="repo-item"
              tabindex="-1"
              onclick={() => handleSelect(repo.nameWithOwner)}
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
              <div class="repo-action">
                <Plus size={14} />
              </div>
            </button>
          {/if}
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .search-input {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 42px;
    padding: 10px 14px;
    border: 1.5px solid var(--border-muted);
    border-radius: 10px;
    background: transparent;
    transition: border-color 0.15s ease;
  }

  .search-input:focus-within {
    border-color: var(--ui-accent);
  }

  :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .search-input input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-md);
    color: var(--text-primary);
    padding: 0;
    font-family: inherit;
  }

  .search-input input::placeholder {
    color: var(--text-faint);
  }

  .search-input input:disabled {
    opacity: 0.6;
  }

  .repo-dropdown {
    max-height: 280px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    box-shadow: var(--shadow-elevated);
    z-index: 100;
    animation: dropdown-in 0.15s ease;
  }

  @keyframes dropdown-in {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .loading-state,
  .empty-state,
  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 20px 16px;
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
    padding: 10px 12px;
    background: none;
    border: none;
    border-radius: 8px;
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

  .keyboard-shortcut {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px 5px;
    background: var(--bg-primary);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    flex-shrink: 0;
    line-height: 1;
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
