<!--
  ReposListView.svelte - Full grid view of all repos with search and pin management.

  Splits the repos into a "Pinned repos" section (by sort order) and an
  "All repos" section for the unpinned rest (by project count). Each card is the
  shared RepoCard, so the grid carries the same repo path label and action row
  as the pinned repos in the projects sidebar.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { Project, RepoHomeItem } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import RepoCard from './RepoCard.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import { useWorkspaceLayer } from '../layout/workspaceLayer';
  import SearchField from '../../shared/SearchField.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import { repoSeedFromNewProjectEvent } from './newProjectEvent';
  import { onViewHidden, selectProject } from '../layout/navigation.svelte';
  import type { RepoSelection } from '../../shared/githubUrl';

  // Served from the shared home-repos cache: revisits paint instantly from
  // memory while the store revalidates in the background.
  let repos = $derived(projectsDataStore.homeRepos);
  let loading = $derived(!projectsDataStore.homeReposLoaded);
  let searchQuery = $state('');
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let showNewProjectModal = $state(false);
  let newProjectInitialRepo = $state<RepoSelection | null>(null);

  // Off screen (home shown over this view, or parked under settings / the
  // diff view), the modal would stay open on top.
  onViewHidden(() => {
    showNewProjectModal = false;
  });

  // The workspace's project layer (App.svelte); the home grid stays mounted
  // under it, so window-level requests need to know which view is on screen.
  const layer = useWorkspaceLayer();

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

  let pinnedRepos = $derived(filteredRepos.filter((r) => r.pinned));
  let unpinnedRepos = $derived(filteredRepos.filter((r) => !r.pinned));

  onMount(() => {
    void projectsDataStore.ensureHomeReposLoaded();

    // This view replaces ProjectHome (and covers ProjectsList, which stays
    // mounted but hidden), so it must handle new-project requests itself.
    const onNewProject = (event: Event) => {
      if (!layer.visible) return;
      newProjectInitialRepo = repoSeedFromNewProjectEvent(event);
      showNewProjectModal = true;
    };
    window.addEventListener('staged:new-project', onNewProject);
    return () => window.removeEventListener('staged:new-project', onNewProject);
  });

  function handleProjectCreated(project: Project) {
    projectsDataStore.projectCreated(project);
    showNewProjectModal = false;
    selectProject(project.id);
  }

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
        <SearchField
          bind:ref={searchInputEl}
          bind:value={searchQuery}
          placeholder="Filter repos..."
          label="Filter repos"
        />
      </div>

      {#if loading}
        <div class="state">Loading repos...</div>
      {:else if filteredRepos.length === 0 && searchQuery.trim()}
        <div class="state">No repos matching "{searchQuery}"</div>
      {:else if filteredRepos.length === 0}
        <div class="state">No repos yet.</div>
      {:else}
        {#if pinnedRepos.length > 0}
          <h2 class="section-title">Pinned repos</h2>
          <div class="repos-grid">
            {#each pinnedRepos as repo (repoKey(repo))}
              <RepoCard {repo} onChange={handleRepoChange} />
            {/each}
          </div>
        {/if}
        {#if unpinnedRepos.length > 0}
          <h2 class="section-title">All repos</h2>
          <div class="repos-grid">
            {#each unpinnedRepos as repo (repoKey(repo))}
              <RepoCard {repo} onChange={handleRepoChange} />
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<NewProjectModal
  open={showNewProjectModal}
  initialRepo={newProjectInitialRepo}
  onCreated={handleProjectCreated}
  onClose={() => (showNewProjectModal = false)}
/>

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

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .section-title {
    margin: 0 0 10px;
    font-size: var(--size-md);
    font-weight: 700;
    color: var(--text-primary);
  }

  .repos-grid + .section-title {
    margin-top: 24px;
  }

  .repos-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    grid-auto-rows: 1fr;
    gap: 12px;
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
