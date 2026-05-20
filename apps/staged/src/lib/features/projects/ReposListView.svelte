<!--
  ReposListView.svelte - Full grid view of all repos with search and pin management.

  Shows pinned repos first (by sort order), then unpinned (by project count).
  Each card has a pin/unpin toggle. Includes a search input for filtering.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowLeft, Pin, PinOff, Search, Download } from 'lucide-svelte';
  import type { RepoHomeItem } from '../../types';
  import * as commands from '../../api/commands';
  import { goHome } from '../layout/navigation.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import {
    badgeFg,
    badgeBg,
    badgeBgHover,
    badgeBorder,
    badgeBorderHover,
  } from '../../shared/badgeColors';
  import { alerts } from '../../shared/alerts.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import ProjectsSidebar from './ProjectsSidebar.svelte';

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
      alerts.show({ tone: 'error', title: 'Failed to load repos', message });
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
      alerts.show({ tone: 'error', title: 'Failed to update pin', message });
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
      alerts.show({ tone: 'error', title: 'Failed to clone repo', message });
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

<div class="repos-list-page">
  <ProjectsSidebar projects={[]} loading={false} error={null} showAllProjectsRow={false} />

  <div class="main-panel">
    <div class="content">
      <div class="header-row">
        <button class="back-btn" onclick={goHome} title="Back to home">
          <ArrowLeft size={16} />
        </button>
        <h1>Repos</h1>
      </div>

      <div class="search-row">
        <div class="search-input-wrapper">
          <Search size={14} />
          <input
            bind:this={searchInputEl}
            type="text"
            placeholder="Filter repos..."
            bind:value={searchQuery}
            class="search-input"
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
                title={subtitle(repo)}
              >
                <button
                  class="pin-toggle"
                  class:pinned={repo.pinned}
                  title={repo.pinned ? 'Unpin repo' : 'Pin repo'}
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
                </button>

                <span class="card-title">{repo.shortName}</span>
                <span class="card-subtitle">{subtitle(repo)}</span>

                {#if !repo.hasLocalClone}
                  <div class="card-footer">
                    <button
                      class="download-btn"
                      title="Clone repo locally"
                      onclick={(e) => handleCloneRepo(repo, e)}
                      disabled={cloningRepos.has(key)}
                    >
                      {#if cloningRepos.has(key)}
                        <Spinner size={14} />
                      {:else}
                        <Download size={14} />
                      {/if}
                    </button>
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
    --sidebar-title-offset: 42px;
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
    padding: var(--sidebar-title-offset) 24px 24px;
    max-width: 900px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .header-row {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 16px;
  }

  .header-row h1 {
    flex: 1;
    margin: 0;
    font-size: var(--size-xl);
    font-weight: 700;
    color: var(--text-primary);
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: none;
    border-radius: 8px;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .back-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
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

  .search-input {
    flex: 1;
    border: none;
    background: none;
    outline: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
  }

  .search-input::placeholder {
    color: var(--text-faint);
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

  .pin-toggle {
    position: absolute;
    top: 8px;
    right: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
    z-index: 2;
  }

  .pin-toggle:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .pin-toggle.pinned {
    color: var(--accent);
  }

  .pin-toggle.pinned:hover:not(:disabled) {
    color: var(--accent);
    background: var(--bg-hover);
  }

  .pin-toggle:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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

  .download-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--card-border);
    border-radius: 6px;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .download-btn:hover:not(:disabled) {
    background: var(--card-bg-hover);
    border-color: var(--card-border-hover);
  }

  .download-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  @media (max-width: 900px) {
    .repos-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  @media (max-width: 640px) {
    .repos-list-page {
      --sidebar-title-offset: 20px;
    }

    .content {
      padding: var(--sidebar-title-offset) 16px 16px;
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
