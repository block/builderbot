<!--
  BoardView.svelte — a pinned board as the detail view: the tag group's board
  (ProjectsBoard) in the place a project normally renders, reached from the
  group's row in the projects sidebar (Settings › Tags › "Show board in
  sidebar").

  The group is fixed by the route, so there is no "Board by" picker; the
  "Hidden columns (n)" control stays, and the collapsed and hidden columns are
  the same per-group preferences the home board keeps. The search field is
  the shared project query (the sidebar's and the home grid's), and the
  filter chips are the home page's, so the board shows the same cards the
  grid would: the columns code already takes the filtered list.

  Once the group is deleted, or its board unpinned, the view leaves the board
  route (navigation.leaveBoard): home, or home under the settings view the
  unpinning was done from; retained under home, its layer is released. Either
  way no empty board is rendered. ⌘N and the "New project" button land here, as in ReposListView,
  since this view replaces ProjectHome and covers ProjectsList.
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import type { Project } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { viewport } from '../../shared/viewport.svelte';
  import SearchField from '../../shared/SearchField.svelte';
  import type { RepoSelection } from '../../shared/githubUrl';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import { useWorkspaceLayer } from '../layout/workspaceLayer';
  import {
    followWhileVisible,
    leaveBoard,
    onViewHidden,
    selectProject,
  } from '../layout/navigation.svelte';
  import NewProjectModal from '../projects/NewProjectModal.svelte';
  import ProjectFilterChips from '../projects/ProjectFilterChips.svelte';
  import { repoSeedFromNewProjectEvent } from '../projects/newProjectEvent';
  import { projectFiltersStore } from '../projects/projectFilters.svelte';
  import { searchOrderHold, visibleProjects } from '../projects/projectSearch';
  import { projectSearchSnapshots } from '../projects/projectSearchContext';
  import { projectSearchIndex } from '../projects/projectSearchIndex.svelte';
  import { projectsListViewState } from '../projects/projectsListViewState.svelte';
  import BoardToolbar from './BoardToolbar.svelte';
  import ProjectsBoard from './ProjectsBoard.svelte';

  interface Props {
    groupId: string;
  }

  let { groupId }: Props = $props();

  // The workspace's project layer (App.svelte): the home grid stays mounted
  // under it, so window-level requests need to know which view is on screen.
  const layer = useWorkspaceLayer();

  let group = $derived(tagsStore.catalog.groupsById.get(groupId) ?? null);
  let projects = $derived(projectsDataStore.liveProjects);
  let loading = $derived(projectsDataStore.loading || !projectsDataStore.loaded);
  let error = $derived(projectsDataStore.error);

  // The shared query, followed key by key only while this view is on screen
  // (ProjectsList and the sidebar make the same call).
  const query = followWhileVisible(() => projectsListViewState.searchQuery);
  let searchActive = $derived(query.current.trim().length > 0);
  const searchSnapshot = projectSearchSnapshots();
  let searchData = $derived(searchActive ? searchSnapshot() : null);
  const holdOrder = searchOrderHold();
  // The chips decide the cards; the query hides the ones it rules out.
  let boardProjects = $derived(projectFiltersStore.filteredProjects);
  let search = $derived(
    holdOrder(
      query.current,
      projectFiltersStore.activeFilters,
      visibleProjects(projects, boardProjects, null, query.current, searchData)
    )
  );
  let searchRank = $derived(search.rank);
  let shownCount = $derived(search.projects.length);

  let searchInputEl = $state<HTMLInputElement | null>(null);
  let showNewProjectModal = $state(false);
  let newProjectInitialRepo = $state<RepoSelection | null>(null);

  onViewHidden(() => {
    showNewProjectModal = false;
  });

  // The board this view shows is no longer offered: its group is gone, or its
  // board unpinned (here or in another window). Leave its route, or release
  // the retained layer, rather than render an empty board.
  $effect(() => {
    if (!tagsStore.loaded || (group && group.showInSidebar)) return;
    untrack(() => leaveBoard(groupId));
  });

  // The cards show repos and status: pull every project's details forward,
  // as the grid does. The store dedupes what it has already fetched.
  $effect(() => {
    if (!projectsDataStore.loaded) return;
    void projectsDataStore.projects;
    untrack(() => void projectsDataStore.ensureProjectsHydrated());
  });

  $effect(() => {
    if (!query.current.trim()) return;
    untrack(() => projectSearchIndex.ensureLoaded());
  });

  onMount(() => {
    void projectsDataStore.ensureLoaded();
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

  function openProject(projectId: string) {
    if (projectsDataStore.isProjectDeleting(projectId)) return;
    selectProject(projectId);
  }
</script>

<TopBarPortal title={group ? group.name : 'Board'} subtitle={group ? 'Board' : null} />

<div class="board-page">
  <div class="main-panel">
    <div class="content">
      {#if error}
        <div class="state error">{error}</div>
      {:else if loading || !tagsStore.loaded}
        <div class="state">Loading projects…</div>
      {:else if group}
        <div class="search-row">
          <SearchField
            bind:ref={searchInputEl}
            bind:value={projectsListViewState.searchQuery}
            placeholder="Search projects, branches, PRs, notes"
            clearable
            onfocus={() => projectSearchIndex.ensureLoaded()}
          />
          {#if !viewport.isMobile}
            <BoardToolbar {group} groupPicker={false} />
          {/if}
        </div>

        <ProjectFilterChips />
        <div aria-live="polite">
          {#if shownCount === 0}
            {#if searchActive && boardProjects.length > 0}
              <div class="state">No projects match "{query.current.trim()}"</div>
            {:else}
              <div class="state">No projects match filters</div>
            {/if}
          {:else if searchActive}
            <span class="sr-only">{shownCount} {shownCount === 1 ? 'project' : 'projects'}</span>
          {/if}
        </div>

        <ProjectsBoard
          {group}
          projects={boardProjects}
          {searchRank}
          onOpen={openProject}
          groupPicker={false}
        />
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
  .board-page {
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

  /* As ProjectsList's content in board mode: no width cap, so the columns
     spread across the window. The bottom inset is the home indicator's. */
  .content {
    flex: 1;
    padding: 24px;
    padding-bottom: calc(24px + env(safe-area-inset-bottom));
    width: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .search-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
  }

  .search-row > :global(.search-field) {
    flex: 1 1 240px;
    min-width: 0;
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  @media (max-width: 640px) {
    .content {
      padding: 16px;
      padding-bottom: calc(16px + env(safe-area-inset-bottom));
    }
  }
</style>
