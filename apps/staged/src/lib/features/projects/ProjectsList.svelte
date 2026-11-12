<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import type { Project } from '../../types';
  import RepoCard from './RepoCard.svelte';
  import { projectDisplayName, projectActivity } from '../../shared/utils';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { disconnectedActivity } from '../../shared/disconnectedSession';
  import { disconnectedSessionsStore } from '../../stores/disconnectedSessions.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import {
    followWhileVisible,
    openSettings,
    selectProject,
    showAllRepos,
  } from '../layout/navigation.svelte';
  import { useWorkspaceLayer } from '../layout/workspaceLayer';
  import NewProjectModal from './NewProjectModal.svelte';
  import { getProjectStatus } from './projectStatus';
  import { workspaceLifecycle } from './workspaceLifecycle.svelte';
  import ProjectStatusIcon from './ProjectStatusIcon.svelte';
  import RemoteProjectIcon from './RemoteProjectIcon.svelte';
  import ProjectCardTagChips from './ProjectCardTagChips.svelte';
  import ProjectCardMenuContent from './ProjectCardMenuContent.svelte';
  import ProjectCardStatusIndicator from './ProjectCardStatusIndicator.svelte';
  import ProjectRepoBadges from './ProjectRepoBadges.svelte';
  import { projectActions } from './projectActions.svelte';
  import { neighbourAfterRemove } from './focusAfterRemove';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import SplashScreen from './SplashScreen.svelte';
  import SearchField from '../../shared/SearchField.svelte';
  import { Button } from '$lib/components/ui/button';
  import { tagsStore } from '../../stores/tags.svelte';
  import ProjectsBoard from '../board/ProjectsBoard.svelte';
  import BoardToolbar from '../board/BoardToolbar.svelte';
  import { resolveBoardGroup } from '../board/boardPrefs';
  import { boardViewPrefs, setProjectsViewMode } from '../board/boardViewPrefs.svelte';

  import {
    captureGridStickyProject,
    finishProjectsListRestore,
    gridStickyProjectId,
    projectsListViewState,
    setProjectsListScrollTop,
  } from './projectsListViewState.svelte';
  import { projectFiltersStore } from './projectFilters.svelte';
  import ProjectFilterChips from './ProjectFilterChips.svelte';
  import ProjectsViewToggle from './ProjectsViewToggle.svelte';
  import {
    projectShortcutNumbers,
    searchOrderHold,
    visibleProjects,
    withStickyProject,
  } from './projectSearch';
  import { projectSearchSnapshots } from './projectSearchContext';
  import { projectSearchIndex } from './projectSearchIndex.svelte';
  import { searchRowOrder } from './searchRowOrder.svelte';
  import ProjectMatchReason from './ProjectMatchReason.svelte';
  import { queryTerms } from '../../shared/textSearch';
  import { repoSeedFromNewProjectEvent } from './newProjectEvent';
  import type { RepoSelection } from '../../shared/githubUrl';
  import { viewport } from '../../shared/viewport.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';

  // Data comes from the shared projectsData store — returning to the landing
  // page paints instantly from memory while the store revalidates in the
  // background. Filters live in the shared projectFiltersStore and the
  // search query in projectsListViewState, so the sidebar sees the same
  // selection and query; scroll restore is view-local but persists across
  // visits via that same state; modal state stays in the component. A project
  // being deleted is already off liveProjects, so its card goes on confirm.
  let projects = $derived(projectsDataStore.liveProjects);
  let projectBranches = $derived(projectsDataStore.branchesByProject);
  let reposByProject = $derived(projectsDataStore.reposByProject);
  let deletingProjectNames = $derived(projectsDataStore.deletingProjectNames);
  let homeRepos = $derived(projectsDataStore.homeRepos);
  // The grid paints as soon as the project list lands. Per-card branch/repo
  // details fill in behind as hydration finishes.
  let loading = $derived(projectsDataStore.loading || !projectsDataStore.loaded);
  let error = $derived(projectsDataStore.error);

  let showNewProjectModal = $state(false);

  // Grid or Board, remembered per window (boardViewPrefs). The board shows the
  // same cards, chips and search as the grid, arranged by one tag group.
  let boardMode = $derived(boardViewPrefs.viewMode === 'board');
  let boardGroup = $derived(resolveBoardGroup(boardViewPrefs.groupId, tagsStore.catalog));
  let board = $state<ReturnType<typeof ProjectsBoard> | null>(null);

  // This view is the workspace's home layer: mounted on the first visit and
  // then kept, covered, under the project view (App.svelte). Anything that
  // used to follow from being mounted — the window shortcuts, the modal, the
  // per-visit capture below — gates on the layer instead.
  const layer = useWorkspaceLayer();

  // Hidden under a project, or parked under settings or the diff view, the
  // modal would stay open above the view on top.
  $effect(() => {
    if (layer.visible) return;
    untrack(() => {
      showNewProjectModal = false;
    });
  });
  let newProjectInitialRepo = $state<RepoSelection | null>(null);
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let isCommandKeyHeld = $state(false);
  let mainPanelEl = $state<HTMLDivElement | null>(null);
  let restoreInProgress = false;
  let restoreToken = 0;
  const projectCardElements = new Map<string, HTMLElement>();

  // The card you just came back from stays in the grid even when the active
  // filters no longer match it, until the filters next change. The rule and
  // its state live in projectsListViewState (captureGridStickyProject,
  // gridStickyProjectId), where the ⌘K palette reads the same card. Captured
  // on each visit: at mount, then before each render that brings the layer
  // back.
  captureGridStickyProject(projectFiltersStore.activeFilters);
  $effect.pre(() => {
    // The line above captures at mount; this re-captures per activation. That
    // includes the mount when the layer is already activated — a cold start
    // into a project, then the first goHome, flips homeLayerRetained after the
    // layer counted that activation — where it repeats the mount's values.
    if (layer.activations === 0) return;
    untrack(() => captureGridStickyProject(projectFiltersStore.activeFilters));
  });

  let effectiveStickyId = $derived(gridStickyProjectId(projectFiltersStore.activeFilters));

  // The query the grid applies: the shared one, followed key by key only
  // while the grid is on screen. Covered by a project, the query changing is
  // the sidebar's search being typed into. Following each keystroke there
  // would restyle and lay out a grid nobody can see, so the grid catches up
  // once typing pauses instead.
  const gridQuery = followWhileVisible(() => projectsListViewState.searchQuery);

  // Chips → sticky card: the cards the grid renders. The query doesn't remove
  // any of them, it hides the ones it rules out. Rendering only the matches
  // destroyed every card a keystroke filtered out (context menu, status icon,
  // repo labels) and rebuilt it on the next backspace.
  let gridProjects = $derived(
    withStickyProject(projects, projectFiltersStore.filteredProjects, effectiveStickyId)
  );
  let searchActive = $derived(gridQuery.current.trim().length > 0);
  // What the query reaches beyond names: branches, PRs, repo labels and
  // titles, prepared once per data change rather than per keystroke. Only
  // built while searching, and the same object again after an update that
  // moved nothing a search reads (a PR status poll), so neither an empty
  // field nor a held query re-runs anything when branches update.
  const searchSnapshot = projectSearchSnapshots();
  let searchData = $derived(searchActive ? searchSnapshot() : null);
  // The cards left on screen, best match first, and why each matched when
  // its name and repos don't say, from one pass. See visibleProjects for the
  // composition. Ranked when the query or the chips are edited and only then:
  // a data change under a held query, a visit's read-state change included,
  // hides or appends cards but never moves one (searchOrderHold).
  const holdOrder = searchOrderHold();
  let search = $derived(
    holdOrder(
      gridQuery.current,
      projectFiltersStore.activeFilters,
      visibleProjects(
        projects,
        projectFiltersStore.filteredProjects,
        effectiveStickyId,
        gridQuery.current,
        searchData
      )
    )
  );
  // What ⌘1–9, the scroll restore and the empty state go by.
  let filteredProjects = $derived(search.projects);
  // Null while the query is blank: every card shows, unordered.
  let searchRank = $derived(search.rank);
  let searchTerms = $derived(queryTerms(gridQuery.current));
  let matchReasons = $derived(search.reasons);
  // Cards keep their {#each} slot whether hidden or moved, so the overlay
  // numbers come from the position on screen (rank order) instead.
  let shortcutNumbers = $derived(projectShortcutNumbers(filteredProjects));
  // The order the {#each} renders the cards in: per keystroke the cards stay
  // put and CSS `order` moves them; once the query settles they are re-keyed
  // into rank order, so tab and reading order match the screen.
  const cardOrder = searchRowOrder(
    () => gridProjects,
    () => searchRank
  );

  // Decides the empty-state copy: blame the query only when the grid would
  // have shown something without it. Judged against the sticky-adjusted base,
  // not the raw chip result, so a sticky card the chips excluded but the query
  // hid is attributed to the query.
  let searchExcludedEverything = $derived(searchActive && gridProjects.length > 0);

  function trackProjectCard(node: HTMLElement, projectId: string) {
    let currentProjectId = projectId;
    projectCardElements.set(currentProjectId, node);

    return {
      update(nextProjectId: string) {
        if (nextProjectId === currentProjectId) return;
        if (projectCardElements.get(currentProjectId) === node) {
          projectCardElements.delete(currentProjectId);
        }
        currentProjectId = nextProjectId;
        projectCardElements.set(currentProjectId, node);
      },
      destroy() {
        if (projectCardElements.get(currentProjectId) === node) {
          projectCardElements.delete(currentProjectId);
        }
      },
    };
  }

  function handleMainPanelScroll() {
    if (!mainPanelEl) return;
    setProjectsListScrollTop(mainPanelEl.scrollTop);
  }

  async function restoreProjectsListPosition() {
    if (!mainPanelEl || restoreInProgress) return;

    restoreInProgress = true;
    const token = ++restoreToken;
    let restored = false;

    try {
      await tick();
      if (token !== restoreToken || !mainPanelEl) return;

      mainPanelEl.scrollTop = projectsListViewState.scrollTop;
      const targetProjectId = projectsListViewState.returnTargetProjectId;
      const targetEl = targetProjectId
        ? (projectCardElements.get(targetProjectId) ?? board?.cardFor(targetProjectId))
        : null;
      targetEl?.scrollIntoView({ block: 'nearest' });
      restored = true;
    } finally {
      if (token === restoreToken) {
        restoreInProgress = false;
        if (restored) {
          finishProjectsListRestore();
        }
      }
    }
  }

  $effect(() => {
    const targetProjectId = projectsListViewState.returnTargetProjectId;
    // Wait for the return-target card's final height, but only while the
    // target is still a project: a delete lands you back here via goHome()
    // with the deleted id as the target, whose card never renders again
    // (hidden while deleting, then pruned along with its hydrated entry).
    const targetHydrated =
      !targetProjectId ||
      !projects.some((p) => p.id === targetProjectId) ||
      projectsDataStore.isProjectHydrated(targetProjectId);
    const readyToDecide =
      projectsListViewState.restorePending &&
      !restoreInProgress &&
      !loading &&
      !error &&
      mainPanelEl &&
      targetHydrated;

    if (!readyToDecide) return;

    // Nothing to restore to — the shared filters can outlive this view and
    // match no project at all. Drop the request rather than leaving it armed:
    // it would otherwise fire whenever the list next became non-empty, jumping
    // scroll to a position captured for a different list.
    if (filteredProjects.length === 0) {
      finishProjectsListRestore();
      return;
    }
    // The target is still a project but has been filtered out of the visible
    // list — by the persisted query (renamed while away, say), or by a chip
    // change between mount and ready, which lifts the sticky exception. Either
    // way the list no longer resembles the one scrollTop was captured
    // against, so don't restore.
    if (
      targetProjectId &&
      projects.some((p) => p.id === targetProjectId) &&
      !filteredProjects.some((p) => p.id === targetProjectId)
    ) {
      finishProjectsListRestore();
      return;
    }

    void restoreProjectsListPosition();
  });

  onMount(() => {
    // Backend/window listeners for the shared data live in the projectsData
    // and projectRunActions stores, started once from App.svelte.
    void projectsDataStore.ensureLoaded();
    void projectsDataStore.ensureHomeReposLoaded();

    const onNewProject = (event: Event) => {
      // The project layer's view answers while this one is hidden under it.
      if (!layer.visible) return;
      newProjectInitialRepo = repoSeedFromNewProjectEvent(event);
      showNewProjectModal = true;
    };
    window.addEventListener('staged:new-project', onNewProject);

    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
    };
  });

  // Pull every project's branches and repos forward off the store's idle drip
  // — the grid renders all of them. An effect rather than onMount so a list
  // change or a cache-stale reload re-kicks the sweep and the loading gate
  // heals itself; the store dedupes projects it has already fetched. Track
  // only `loaded` and `projects`: the sweep reads the hydrated-project map
  // synchronously, so tracking it too would re-run this once per project.
  $effect(() => {
    if (!projectsDataStore.loaded) return;
    void projectsDataStore.projects;
    untrack(() => void projectsDataStore.ensureProjectsHydrated());
  });

  // Titles (notes, project notes, reviews) come from the lazily loaded search
  // index: fetch it once a query starts, if focusing the field didn't already.
  // Asked again on every keystroke so a load that failed mid-search retries
  // rather than leaving titles unmatched; writes during the search wait for
  // the index's coalesced refetch instead (projectSearchIndex).
  $effect(() => {
    if (!gridQuery.current.trim()) return;
    untrack(() => projectSearchIndex.ensureLoaded());
  });

  // Keep run-action state hydrated for the status badges; the store call
  // dedupes branches it has already queried.
  $effect(() => {
    projectRunActionsStore
      .hydrateFromProjectBranches(projectsDataStore.branchesByProject)
      .catch(console.error);
  });

  function handleProjectCreated(project: Project) {
    projectsDataStore.projectCreated(project);
    showNewProjectModal = false;
    selectProject(project.id);
  }

  function isProjectDeleting(projectId: string): boolean {
    return projectsDataStore.isProjectDeleting(projectId);
  }

  // The card goes as soon as the remove is confirmed, taking focus with it, so
  // focus moves on to the card that takes its place.
  function removeProject(project: Project) {
    const focusAfterRemove = neighbourAfterRemove(
      filteredProjects.map((p) => p.id),
      project.id,
      (projectId) =>
        searchRank === null || searchRank.has(projectId)
          ? projectCardElements.get(projectId)?.querySelector<HTMLElement>('.project-card')
          : null
    );
    void projectActions.requestRemoveProject(project, focusAfterRemove);
  }

  function openProject(projectId: string) {
    if (isProjectDeleting(projectId)) return;
    if (mainPanelEl) {
      setProjectsListScrollTop(mainPanelEl.scrollTop);
    }
    selectProject(projectId);
  }

  function verifyCommandKeyState(e: KeyboardEvent | MouseEvent) {
    // Verify the command key is actually held down by checking the event's metaKey/ctrlKey
    const actuallyHeld = e.metaKey || e.ctrlKey;
    if (isCommandKeyHeld && !actuallyHeld) {
      isCommandKeyHeld = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    // Still mounted but hidden: under a project, settings or the diff view.
    if (!layer.visible) return;
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    // Typing in an input must not trigger shortcuts, but the natural flow for
    // the search box is type a query → ⌘1 to open the first hit, so modified
    // keys from it fall through: the ⌘ press itself shows the overlay and the
    // digit opens the card. Unmodified digits still reach the input.
    const fromSearchWithModifier = target === searchInputEl && (e.metaKey || e.ctrlKey);
    if (isInput && !fromSearchWithModifier) return;

    // Track command key state
    if (e.metaKey || e.ctrlKey) {
      isCommandKeyHeld = true;
    } else {
      // Any non-command key press while we think command is held means it's not
      verifyCommandKeyState(e);
    }

    // Command+1-9 to open projects by number. Grid only: the numbers are
    // the grid's reading order, which the board's columns don't share, and
    // only grid cards show them, so on the board the shortcut would open a
    // project the user can't see numbered anywhere.
    if (!boardMode && (e.metaKey || e.ctrlKey) && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      const index = parseInt(e.key) - 1;
      if (index < filteredProjects.length) {
        openProject(filteredProjects[index].id);
      }
    }
  }

  function handleKeyup(e: KeyboardEvent) {
    // Verify the actual state first
    verifyCommandKeyState(e);
    if (!e.metaKey && !e.ctrlKey) {
      isCommandKeyHeld = false;
    }
  }

  function handleBlur() {
    // Reset command key state when window loses focus
    // This handles cases like Command+Tab where keyup isn't received
    isCommandKeyHeld = false;
  }

  function handleMouseMove(e: MouseEvent) {
    // Verify command key state on mouse movement
    // This catches cases where the state got stuck (e.g., Command+Tab back to app)
    if (isCommandKeyHeld) {
      verifyCommandKeyState(e);
    }
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  onkeyup={handleKeyup}
  onblur={handleBlur}
  onmousemove={handleMouseMove}
/>

<TopBarPortal title="Projects" rightActions={rootTopBarActions} />

{#snippet rootTopBarActions()}
  <span class="inline-flex" title={viewport.showShortcutHints ? 'New project (⌘N)' : 'New project'}>
    <Button
      variant="ghost"
      size="icon-xs"
      class="top-bar-action gap-1.5"
      aria-label="New project"
      onclick={() => window.dispatchEvent(new CustomEvent('staged:new-project'))}
    >
      <Plus size={14} />
      <span class="top-bar-action-label mobile-only">New project</span>
    </Button>
  </span>

  <Button
    variant="ghost"
    size="icon-xs"
    class="top-bar-action"
    title={viewport.showShortcutHints ? 'Settings (⌘,)' : 'Settings'}
    aria-label="Settings"
    onclick={() => openSettings()}
  >
    <SlidersHorizontal size={14} />
  </Button>
{/snippet}

<div class="projects-list-page">
  <div class="main-panel" bind:this={mainPanelEl} onscroll={handleMainPanelScroll}>
    <div
      class="content"
      class:empty-layout={!loading && !error && projects.length === 0}
      class:board={boardMode}
    >
      {#if error}
        <div class="state error">{error}</div>
      {:else if loading}
        <div class="state">Loading projects…</div>
      {:else if projects.length === 0}
        <SplashScreen
          onCreated={handleProjectCreated}
          requestOpen={showNewProjectModal && projects.length === 0}
          onFormOpenChange={(open) => (showNewProjectModal = open)}
        />
      {:else}
        {#if homeRepos.length > 0}
          <div class="repos-section">
            <div class="repos-header">
              <h2 class="repos-title">Repos</h2>
              <Button
                variant="ghost"
                size="sm"
                class="touch-hit h-7 text-muted-foreground"
                onclick={showAllRepos}>View all</Button
              >
            </div>
            <div class="repos-scroll-row">
              {#each homeRepos as repo (repo.githubRepo + ':' + repo.subpath)}
                <RepoCard {repo} onChange={() => projectsDataStore.refreshHomeRepos()} />
              {/each}
            </div>
          </div>
        {/if}

        <div class="search-row">
          <SearchField
            bind:ref={searchInputEl}
            bind:value={projectsListViewState.searchQuery}
            placeholder="Search projects, branches, PRs, notes"
            clearable
            onfocus={() => projectSearchIndex.ensureLoaded()}
            onkeydown={cardOrder.fieldKeydown}
          />
          <ProjectsViewToggle value={boardViewPrefs.viewMode} onChange={setProjectsViewMode} />
          {#if boardMode && boardGroup && tagsStore.loaded && !viewport.isMobile}
            <BoardToolbar group={boardGroup} />
          {/if}
        </div>

        <ProjectFilterChips />
        <!-- Always in the DOM so the live region exists before its text
             changes; screen readers only announce updates to a region they
             have already seen. -->
        <div aria-live="polite">
          {#if filteredProjects.length === 0}
            {#if searchExcludedEverything}
              <div class="state">
                No projects match "{gridQuery.current.trim()}"
              </div>
            {:else}
              <div class="state">No projects match filters</div>
            {/if}
          {:else if searchActive}
            <!-- Announce the grid narrowing as the user types, not just when
                 it empties. Search-only so ordinary loads stay quiet. -->
            <span class="sr-only">
              {filteredProjects.length}
              {filteredProjects.length === 1 ? 'project' : 'projects'}
            </span>
          {/if}
        </div>
        {#if boardMode}
          <ProjectsBoard
            bind:this={board}
            group={boardGroup}
            projects={gridProjects}
            {searchRank}
            onOpen={openProject}
          />
        {:else}
          <div class="projects-grid" onpointerdowncapture={cardOrder.rowsPointerdown}>
            {#each cardOrder.rows as project (project.id)}
              {@const status = getProjectStatus(
                project.id,
                deletingProjectNames,
                projectBranches.get(project.id) || [],
                workspaceLifecycle.getWorktreeErrors(),
                projectsDataStore.transferringProjects
              )}
              {@const hydrated = projectsDataStore.isProjectHydrated(project.id)}
              {@const repos = reposByProject.get(project.id) ?? []}
              {@const sessionTypes = projectStateStore.getRunningSessionTypes(project.id)}
              {@const reconcileState =
                status.kind === 'disconnected'
                  ? disconnectedSessionsStore.projectReconcileState(project.id)
                  : null}
              {@const activity =
                projectActivity(sessionTypes, status.runActionPhase) ||
                (status.kind === 'disconnected' ? disconnectedActivity(reconcileState) : '')}
              {@const shortcut = shortcutNumbers.get(project.id)}
              {@const matchReason = matchReasons.get(project.id)}
              <div
                class="project-card-wrapper"
                hidden={searchRank !== null && !searchRank.has(project.id)}
                style:order={cardOrder.orderOf(project.id)}
                use:trackProjectCard={project.id}
              >
                <ContextMenu.Root>
                  <ContextMenu.Trigger class="contents">
                    <button class="project-card" onclick={() => openProject(project.id)}>
                      {#if viewport.showShortcutHints && isCommandKeyHeld && shortcut !== undefined}
                        <div class="keyboard-shortcut-overlay">
                          <span class="command-icon">⌘</span>
                          <span class="number">{shortcut}</span>
                        </div>
                      {/if}
                      <ProjectCardStatusIndicator projectId={project.id} {status} />
                      <div class="card-header">
                        <!-- Chosen statuses paint at once; only the PR status
                           waits on branches. -->
                        <ProjectStatusIcon
                          {project}
                          branches={projectBranches.get(project.id) || []}
                          {hydrated}
                          size={16}
                        />
                        <span>{projectDisplayName(project)}</span>
                      </div>
                      <ProjectCardTagChips projectId={project.id} />
                      {#if matchReason}
                        <div class="card-match">
                          <ProjectMatchReason reason={matchReason} terms={searchTerms} />
                        </div>
                      {/if}
                      {#if activity}
                        <div class="card-activity">
                          {activity}
                        </div>
                      {/if}
                      <div class="card-footer">
                        {#if project.location === 'remote'}
                          <RemoteProjectIcon size={14} />
                        {/if}
                        <ProjectRepoBadges {project} {repos} />
                      </div>
                    </button>
                  </ContextMenu.Trigger>
                  <ProjectCardMenuContent
                    {project}
                    transferring={status.kind === 'transferring'}
                    onRemove={() => removeProject(project)}
                  />
                </ContextMenu.Root>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<NewProjectModal
  open={showNewProjectModal && projects.length > 0}
  initialRepo={newProjectInitialRepo}
  onCreated={handleProjectCreated}
  onClose={() => (showNewProjectModal = false)}
/>

<style>
  .projects-list-page {
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

  /* The bottom inset is the home indicator's (App.svelte's main leaves it to
     the panes' content). */
  .content {
    flex: 1;
    padding: 24px;
    padding-bottom: calc(24px + env(safe-area-inset-bottom));
    max-width: 900px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  /* The board spreads its columns across the window. */
  .content.board {
    max-width: none;
  }

  .content.empty-layout {
    max-width: none;
    padding: 0;
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  .repos-section {
    margin-bottom: 24px;
  }

  .repos-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .repos-title {
    margin: 0;
    font-size: var(--size-lg);
    font-weight: 700;
    color: var(--text-primary);
  }

  .repos-scroll-row {
    display: flex;
    gap: 10px;
    overflow-x: auto;
    padding-bottom: 4px;
    scrollbar-width: thin;
    scrollbar-color: var(--border-muted) transparent;
  }

  .repos-scroll-row > :global(.repo-card) {
    width: 200px;
    flex-shrink: 0;
  }

  .repos-scroll-row::-webkit-scrollbar {
    height: 4px;
  }

  .repos-scroll-row::-webkit-scrollbar-track {
    background: transparent;
  }

  .repos-scroll-row::-webkit-scrollbar-thumb {
    background: var(--border-muted);
    border-radius: 2px;
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

  /* A grid, not just for the columns: until a query settles, each shown
     card's inline `order` lays the cards out by rank without moving them in
     the DOM. */
  .projects-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    grid-auto-rows: auto;
    gap: 12px;
  }

  .project-card-wrapper {
    display: flex;
    flex-direction: column;
  }

  /* The display above outranks the UA's [hidden] rule, so the search's
     `hidden` restates display: none here rather than lean on Tailwind
     preflight's `[hidden] { display: none !important }`. */
  .project-card-wrapper[hidden] {
    display: none;
  }

  .project-card-wrapper .project-card {
    flex: 1;
  }

  .card-activity {
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .project-card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
    text-align: left;
    background: var(--bg-primary);
    border: none;
    border-radius: 10px;
    padding: 16px;
    color: inherit;
    cursor: pointer;
    transition: background-color 0.15s ease;
    /* Skip style, layout and paint for cards outside the viewport: the grid
       mounts every project at once and most sit below the fold. This goes on
       the button rather than the wrapper because the paint containment it
       implies clips descendants, and on the wrapper that clipped the button's
       focus ring. The placeholder is the content-box height (68px + 32px
       padding = 100px) of the mean three-column card on a large real data set
       (81–123px, median 102); `auto` keeps each card's own height once it has
       rendered, so the scrollbar and scroll restore only estimate until then.
       Older WebKit ignores the property. */
    content-visibility: auto;
    contain-intrinsic-size: auto 68px;
  }

  .project-card:hover {
    background-color: var(--bg-hover);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-primary);
    font-size: var(--size-lg);
    font-weight: 600;
    padding-right: 24px;
  }

  .card-header :global(svg) {
    flex-shrink: 0;
  }

  .card-match {
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  /* Pinned to the bottom of the card. A remote project's cloud leads the repo
     line in a column as wide as the header's status icon, so it sits under
     that icon and the repos line up with the name. */
  .card-footer {
    margin-top: auto;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .card-footer > :global(.remote-project-icon) {
    width: 16px;
  }

  .keyboard-shortcut-overlay {
    position: absolute;
    top: 8px;
    right: 8px;
    display: flex;
    align-items: center;
    gap: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-emphasis);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-primary);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
    z-index: 10;
    pointer-events: none;
  }

  .keyboard-shortcut-overlay .command-icon {
    color: var(--ui-accent);
    font-size: var(--size-sm);
  }

  .keyboard-shortcut-overlay .number {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    color: var(--ui-accent);
  }

  @media (max-width: 900px) {
    .projects-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    /* Two-column cards are shorter (median 81px wrapper on the same data set):
       49px + 32px padding. WebKit has no scroll anchoring, so a placeholder
       that is consistently too tall drifts the viewport when skipped rows
       above it render while scrolling back up. */
    .project-card {
      contain-intrinsic-size: auto 49px;
    }
  }

  @media (max-width: 640px) {
    .content {
      padding: 16px;
      padding-bottom: calc(16px + env(safe-area-inset-bottom));
    }

    .projects-grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 10px;
    }

    .project-card {
      padding: 14px;
      /* Median one-column wrapper is 75px: 47px + 28px padding. */
      contain-intrinsic-size: auto 47px;
    }

    .card-header {
      font-size: var(--size-md);
    }
  }
</style>
