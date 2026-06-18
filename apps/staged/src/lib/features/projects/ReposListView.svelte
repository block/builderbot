<!--
  ReposListView.svelte - Full grid view of all repos with search and pin management.

  Shows pinned repos first (by sort order), then unpinned (by project count).
  Each card has a pin/unpin toggle. Includes a search input for filtering.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Pin from '@lucide/svelte/icons/pin';
  import PinOff from '@lucide/svelte/icons/pin-off';
  import Search from '@lucide/svelte/icons/search';
  import Download from '@lucide/svelte/icons/download';
  import type { RepoHomeItem } from '../../types';
  import * as commands from '../../api/commands';
  import { darkMode } from '../../stores/isDark.svelte';
  import {
    badgeFg,
    badgeBg,
    badgeBgHover,
    badgeBorder,
    badgeBorderHover,
  } from '../../shared/badgeColors';
  import { toast } from 'svelte-sonner';
  import Spinner from '../../shared/Spinner.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';

  let repos = $state<RepoHomeItem[]>([]);
  let loading = $state(true);
  let searchQuery = $state('');
  let togglingPin = $state<Set<string>>(new Set());
  let cloningRepos = $state<Set<string>>(new Set());
  let searchInputEl = $state<HTMLInputElement | null>(null);

  function repoKey(r: RepoHomeItem): string {
    return `${r.githubRepo}:${r.subpath}`;
  }

  let filteredRepos = $derived.by(() => {
    if (!searchQuery.trim()) return repos;
    const q = searchQuery.toLowerCase().trim();
    return repos.filter(
      (r) =>
        r.shortName.toLowerCase().includes(q) ||
        r.githubRepo.toLowerCase().includes(q) ||
        (r.subpath && r.subpath.toLowerCase().includes(q))
    );
  });

  onMount(() => {
    loadRepos();
  });

  async function loadRepos() {
    loading = true;
    try {
      repos = await commands.listReposForHome();
    } catch (e) {
      console.error('[ReposListView] Failed to load repos:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Failed to load repos', { description: message });
    } finally {
      loading = false;
    }
  }

  async function togglePin(repo: RepoHomeItem, e: MouseEvent) {
    e.stopPropagation();
    const key = repoKey(repo);
    if (togglingPin.has(key)) return;

    togglingPin = new Set(togglingPin).add(key);
    try {
      if (repo.pinned) {
        await commands.unpinRepo(repo.githubRepo, repo.subpath);
      } else {
        await commands.pinRepo(repo.githubRepo, repo.subpath);
      }
      await loadRepos();
      window.dispatchEvent(new CustomEvent('staged:pinned-repos-changed'));
    } catch (e) {
      console.error('[ReposListView] Failed to toggle pin:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Failed to update pin', { description: message });
    } finally {
      const next = new Set(togglingPin);
      next.delete(key);
      togglingPin = next;
    }
  }

  async function handleCloneRepo(repo: RepoHomeItem, e: MouseEvent) {
    e.stopPropagation();
    const key = repoKey(repo);
    if (cloningRepos.has(key)) return;

    cloningRepos = new Set(cloningRepos).add(key);
    try {
      await commands.cloneRepoLocally(repo.githubRepo);
      await loadRepos();
    } catch (e) {
      console.error('[ReposListView] Failed to clone repo:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Failed to clone repo', { description: message });
    } finally {
      const next = new Set(cloningRepos);
      next.delete(key);
      cloningRepos = next;
    }
  }

  function subtitle(repo: RepoHomeItem): string {
    const base = repo.githubRepo;
    if (repo.subpath) return `${base}/${repo.subpath}`;
    return base;
  }
</script>

<TopBarPortal title="Repos" />

<div class="repos-list-page">
  <div class="main-panel">
    <div class="content">
      <div class="search-row">
        <div class="search-input-wrapper">
          <Search size={14} />
          <Input
            bind:ref={searchInputEl}
            type="text"
            placeholder="Filter repos..."
            bind:value={searchQuery}
            class="border-0 bg-transparent shadow-none px-0 py-0 h-auto min-h-0 focus-visible:ring-0 focus-visible:border-0 text-sm"
          />
        </div>
      </div>

      {#if loading}
        <div class="state">Loading repos...</div>
      {:else if filteredRepos.length === 0 && searchQuery.trim()}
        <div class="state">No repos matching "{searchQuery}"</div>
      {:else if filteredRepos.length === 0}
        <div class="state">No repos yet.</div>
      {:else}
        <div class="repos-grid">
          {#each filteredRepos as repo (repoKey(repo))}
            {@const accent = badgeFg(repo.hue, darkMode.value)}
            {@const bg = badgeBg(repo.hue, darkMode.value)}
            {@const bgHover = badgeBgHover(repo.hue, darkMode.value)}
            {@const border = badgeBorder(repo.hue, darkMode.value)}
            {@const borderHover = badgeBorderHover(repo.hue, darkMode.value)}
            {@const key = repoKey(repo)}
            <div class="repo-card-wrapper">
              <div
                class="repo-card"
                style="--accent: {accent}; --card-bg: {bg}; --card-bg-hover: {bgHover}; --card-border: {border}; --card-border-hover: {borderHover};"
              >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class={[
                    'absolute top-2 right-2 z-[2] size-7 rounded-md bg-transparent hover:bg-[var(--bg-hover)] [&_svg]:!size-3.5',
                    repo.pinned
                      ? 'text-[var(--accent)] hover:text-[var(--accent)]'
                      : 'text-[var(--text-faint)] hover:text-foreground',
                  ]}
                  title={repo.pinned ? 'Unpin repo' : 'Pin repo'}
                  aria-label={repo.pinned ? 'Unpin repo' : 'Pin repo'}
                  onclick={(e) => togglePin(repo, e)}
                  disabled={togglingPin.has(key)}
                >
                  {#if togglingPin.has(key)}
                    <Spinner size={14} />
                  {:else if repo.pinned}
                    <Pin size={14} />
                  {:else}
                    <PinOff size={14} />
                  {/if}
                </Button>

                <span class="card-title" title={repo.shortName}>{repo.shortName}</span>
                <span class="card-subtitle" title={subtitle(repo)}>{subtitle(repo)}</span>

                {#if !repo.hasLocalClone}
                  <div class="card-footer">
                    <Button
                      variant="outline"
                      size="icon-sm"
                      class="size-7 rounded-md border-[var(--card-border)] bg-transparent text-[var(--accent)] shadow-none hover:border-[var(--card-border-hover)] hover:bg-[var(--card-bg-hover)] hover:text-[var(--accent)] [&_svg]:!size-3.5"
                      title="Clone repo locally"
                      aria-label="Clone repo locally"
                      onclick={(e) => handleCloneRepo(repo, e)}
                      disabled={cloningRepos.has(key)}
                    >
                      {#if cloningRepos.has(key)}
                        <Spinner size={14} />
                      {:else}
                        <Download size={14} />
                      {/if}
                    </Button>
                  </div>
                {/if}
              </div>
              {#if repo.pinned}
                <div class="card-label">Pinned</div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .repos-list-page {
    flex: 1;
    min-height: 0;
    display: flex;
    min-width: 0;
    background-color: var(--bg-chrome);
    overflow: hidden;
  }

  .main-panel {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow-y: scroll;
  }

  .content {
    flex: 1;
    padding: 24px;
    max-width: 900px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .search-row {
    margin-bottom: 16px;
  }

  .search-input-wrapper {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-elevated);
    color: var(--text-muted);
    transition: border-color 0.15s ease;
  }

  .search-input-wrapper:focus-within {
    border-color: var(--border-emphasis);
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .repos-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    grid-auto-rows: 1fr;
    gap: 12px;
  }

  .repo-card-wrapper {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .repo-card-wrapper .repo-card {
    flex: 1;
  }

  .card-label {
    color: var(--text-faint);
    font-size: var(--size-xs);
    padding: 0 4px;
  }

  .repo-card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    text-align: left;
    min-height: 120px;
    padding: 14px;
    border: 1px solid var(--card-border);
    border-radius: 10px;
    background: var(--card-bg);
    color: inherit;
    transition: all 0.15s ease;
    box-sizing: border-box;
  }

  .card-title {
    font-size: var(--size-md);
    font-weight: 700;
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-right: 32px;
  }

  .card-subtitle {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-footer {
    margin-top: auto;
    display: flex;
    align-items: center;
    min-height: 20px;
  }

  @media (max-width: 900px) {
    .repos-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  @media (max-width: 640px) {
    .content {
      padding: 16px;
    }

    .repos-grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 10px;
    }

    .repo-card {
      min-height: 104px;
    }
  }
</style>
