<!--
  ReposListView.svelte - Full grid view of all repos with search and pin management.

  Shows pinned repos first (by sort order), then unpinned (by project count).
  Each card is the shared RepoCard, so the grid carries the same repo path label
  and action row as the pinned repos in the projects sidebar.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Search from '@lucide/svelte/icons/search';
  import type { RepoHomeItem } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import RepoCard from './RepoCard.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import { Input } from '$lib/components/ui/input';

  // Served from the shared home-repos cache: revisits paint instantly from
  // memory while the store revalidates in the background.
  let repos = $derived(projectsDataStore.homeRepos);
  let loading = $derived(!projectsDataStore.homeReposLoaded);
  let searchQuery = $state('');
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
    void projectsDataStore.ensureHomeReposLoaded();
  });

  // RepoCard pins, unpins and clones in place; refetching through the store
  // updates every consumer (sidebar pinned list, landing-page strip), so no
  // window event is needed here.
  function handleRepoChange() {
    void projectsDataStore.refreshHomeRepos();
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
            <div class="repo-card-wrapper">
              <RepoCard {repo} onChange={handleRepoChange} />
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

  .repo-card-wrapper > :global(.repo-card) {
    flex: 1;
  }

  .card-label {
    color: var(--text-faint);
    font-size: var(--size-xs);
    padding: 0 4px;
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
  }
</style>
