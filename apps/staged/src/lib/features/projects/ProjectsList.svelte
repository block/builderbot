<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { fade } from 'svelte/transition';
  import Mail from '@lucide/svelte/icons/mail';
  import Plus from '@lucide/svelte/icons/plus';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import type { Project } from '../../types';
  import RepoCard from './RepoCard.svelte';
  import { projectDisplayName, projectActivity } from '../../shared/utils';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { openSettings, selectProject, showAllRepos } from '../layout/navigation.svelte';
  import { useWorkspaceLayer } from '../layout/workspaceLayer';
  import NewProjectModal from './NewProjectModal.svelte';
  import { getProjectStatus } from './projectStatus';
  import { workspaceLifecycle } from './workspaceLifecycle.svelte';
  import ProjectStatusIcon from './ProjectStatusIcon.svelte';
  import ProjectStatusSubmenu from './ProjectStatusSubmenu.svelte';
  import { projectActions } from './projectActions.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import SplashScreen from './SplashScreen.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import SearchField from '../../shared/SearchField.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { Button } from '$lib/components/ui/button';

  import {
    finishProjectsListRestore,
    projectsListViewState,
    setProjectsListScrollTop,
  } from './projectsListViewState.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { badgeBg, badgeFg } from '../../shared/badgeColors';
  import { projectFiltersStore } from './projectFilters.svelte';
  import ProjectFilterChips from './ProjectFilterChips.svelte';
  import { visibleProjects, withStickyProject } from './projectSearch';
  import { repoSeedFromNewProjectEvent } from './newProjectEvent';
  import type { RepoSelection } from '../../shared/githubUrl';
  import { viewport } from '../../shared/viewport.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';

  // Data comes from the shared projectsData store — returning to the landing
  // page paints instantly from memory while the store revalidates in the
  // background. Filters live in the shared projectFiltersStore and the
  // search query in projectsListViewState, so the sidebar sees the same
  // selection and query; scroll restore is view-local but persists across
  // visits via that same state; modal state stays in the component.
  let projects = $derived(projectsDataStore.projects);
  let projectBranches = $derived(projectsDataStore.branchesByProject);
  let reposByProject = $derived(projectsDataStore.reposByProject);
  let deletingProjectNames = $derived(projectsDataStore.deletingProjectNames);
  let homeRepos = $derived(projectsDataStore.homeRepos);
  // The grid paints as soon as the project list lands. Per-card branch/repo
  // details fill in behind as hydration finishes.
  let loading = $derived(projectsDataStore.loading || !projectsDataStore.loaded);
  let error = $derived(projectsDataStore.error);

  let showNewProjectModal = $state(false);

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
  // filters no longer match it: visiting a project marks it read, so with
  // Unread active the card returnTargetProjectId points at — the one the
  // scroll restore is aiming for — would otherwise be gone on arrival. Same
  // exception the sidebar makes for the selected project. Captured on each
  // visit — at mount, then before each render that brings the layer back —
  // but only while a restore is actually pending: returnTargetProjectId is
  // never cleared, so an ordinary visit — landing → settings → back, where
  // requestProjectsListRestore(null) early-returns — would otherwise pin a
  // project from a previous visit. restorePending is still true here; the
  // effect that finishes the restore runs after.
  function pendingStickyProjectId(): string | null {
    return projectsListViewState.restorePending
      ? projectsListViewState.returnTargetProjectId
      : null;
  }
  let stickyProjectId = $state(pendingStickyProjectId());
  // The exception lasts only until the first filter change, so a deliberate
  // re-filter hides the card. The store replaces the Set on every change, so
  // identity is the signal — derived rather than tracked in an effect, which
  // would paint the stale card once under the new filters before removing it.
  let filtersAtVisit = $state(projectFiltersStore.activeFilters);
  $effect.pre(() => {
    // The mount capture is the initialiser above; this re-captures per return.
    if (layer.activations === 0) return;
    untrack(() => {
      stickyProjectId = pendingStickyProjectId();
      filtersAtVisit = projectFiltersStore.activeFilters;
    });
  });

  let effectiveStickyId = $derived(
    projectFiltersStore.activeFilters === filtersAtVisit ? stickyProjectId : null
  );

  // Chips → sticky card → search; see visibleProjects for the composition.
  let filteredProjects = $derived(
    visibleProjects(
      projects,
      projectFiltersStore.filteredProjects,
      effectiveStickyId,
      projectsListViewState.searchQuery
    )
  );

  let searchActive = $derived(projectsListViewState.searchQuery.trim().length > 0);
  // Decides the empty-state copy: blame the query only when the grid would
  // have shown something without it. Judged against the sticky-adjusted base,
  // not the raw chip result, so a sticky card the chips excluded but the query
  // hid is attributed to the query.
  let searchExcludedEverything = $derived(
    searchActive &&
      withStickyProject(projects, projectFiltersStore.filteredProjects, effectiveStickyId).length >
        0
  );

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
      const targetEl = targetProjectId ? projectCardElements.get(targetProjectId) : null;
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
    // with the deleted id as the target, and applyProjectList has already
    // pruned it from the hydrated set, so it would otherwise never satisfy.
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

    // Command+1-9 to open projects by number
    if ((e.metaKey || e.ctrlKey) && /^[1-9]$/.test(e.key)) {
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
      class="max-md:size-10 [&_svg]:size-3.5"
      aria-label="New project"
      onclick={() => window.dispatchEvent(new CustomEvent('staged:new-project'))}
    >
      <Plus size={14} />
    </Button>
  </span>

  <Button
    variant="ghost"
    size="icon-xs"
    class="max-md:size-10 [&_svg]:size-3.5"
    title={viewport.showShortcutHints ? 'Settings (⌘,)' : 'Settings'}
    aria-label="Settings"
    onclick={() => openSettings()}
  >
    <SlidersHorizontal size={14} />
  </Button>
{/snippet}

<div class="projects-list-page">
  <div class="main-panel" bind:this={mainPanelEl} onscroll={handleMainPanelScroll}>
    <div class="content" class:empty-layout={!loading && !error && projects.length === 0}>
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
                class="h-7 text-muted-foreground"
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
            placeholder="Search projects"
            clearable
          />
        </div>

        <ProjectFilterChips />
        <!-- Always in the DOM so the live region exists before its text
             changes; screen readers only announce updates to a region they
             have already seen. -->
        <div aria-live="polite">
          {#if filteredProjects.length === 0}
            {#if searchExcludedEverything}
              <div class="state">
                No projects match "{projectsListViewState.searchQuery.trim()}"
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
        <div class="projects-grid">
          {#each filteredProjects as project, index (project.id)}
            {@const status = getProjectStatus(
              project.id,
              deletingProjectNames,
              projectBranches.get(project.id) || [],
              workspaceLifecycle.getWorktreeErrors()
            )}
            {@const busy = status.kind === 'running' || status.kind === 'backgroundHold'}
            {@const busyTitle =
              status.kind === 'backgroundHold' ? 'Waiting on background work' : undefined}
            {@const hydrated = projectsDataStore.isProjectHydrated(project.id)}
            {@const repos = reposByProject.get(project.id) ?? []}
            {@const sessionTypes = projectStateStore.getRunningSessionTypes(project.id)}
            {@const activity = projectActivity(sessionTypes, status.runActionPhase)}
            <div class="project-card-wrapper" use:trackProjectCard={project.id}>
              <ContextMenu.Root>
                <ContextMenu.Trigger class="contents" disabled={status.kind === 'deleting'}>
                  <button
                    class="project-card"
                    class:deleting={status.kind === 'deleting'}
                    onclick={() => openProject(project.id)}
                    disabled={status.kind === 'deleting'}
                    title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
                  >
                    {#if viewport.showShortcutHints && isCommandKeyHeld && index < 9}
                      <div class="keyboard-shortcut-overlay">
                        <span class="command-icon">⌘</span>
                        <span class="number">{index + 1}</span>
                      </div>
                    {/if}
                    {#if status.runActionPhase === 'running' && busy}
                      <div
                        class="status-indicator wave-spinner"
                        title={busyTitle}
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      >
                        <SineWave size={14} />
                        <Spinner size={14} />
                      </div>
                    {:else if status.runActionPhase === 'running'}
                      <div
                        class="status-indicator wave"
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      >
                        <SineWave size={14} />
                      </div>
                    {:else if status.kind === 'runAction' || busy}
                      <div
                        class="status-indicator spinner"
                        title={busyTitle}
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      >
                        <Spinner size={14} />
                      </div>
                    {:else if status.kind === 'unread'}
                      <div
                        class="status-indicator unread-dot"
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      ></div>
                    {/if}
                    <div class="card-header">
                      <!-- Remote cards and chosen statuses paint at once; only
                           the PR status waits on branches. -->
                      <ProjectStatusIcon
                        {project}
                        branches={projectBranches.get(project.id) || []}
                        {hydrated}
                        size={16}
                      />
                      <span>{projectDisplayName(project)}</span>
                    </div>
                    {#if activity}
                      <div class="card-activity">
                        {activity}
                      </div>
                    {/if}
                    {#if status.kind === 'deleting'}
                      <div class="deleting-pill" role="status" aria-live="polite">Deleting…</div>
                    {/if}
                    <div class="repo">
                      {#if repos.length > 0}
                        {#each [...repos].sort((a, b) => {
                          const aKey = a.subpath ? `${a.githubRepo}/${a.subpath}` : a.githubRepo;
                          const bKey = b.subpath ? `${b.githubRepo}/${b.subpath}` : b.githubRepo;
                          return aKey.localeCompare(bKey);
                        }) as r}
                          {@const badge = repoBadgeStore.lookup(r.githubRepo, r.subpath)}
                          {#if badge}
                            <span
                              class="repo-badge-label"
                              style="background: {badgeBg(
                                badge.hue,
                                darkMode.value
                              )}; color: {badgeFg(badge.hue, darkMode.value)};"
                            >
                              <RepoLabel
                                githubRepo={r.headRepo ?? r.githubRepo}
                                subpath={r.subpath}
                              />
                            </span>
                          {:else}
                            <span class="repo-line">
                              <RepoLabel
                                githubRepo={r.headRepo ?? r.githubRepo}
                                subpath={r.subpath}
                              />
                            </span>
                          {/if}
                        {/each}
                      {:else if project.githubRepo}
                        {@const badge = repoBadgeStore.lookup(project.githubRepo, project.subpath)}
                        {#if badge}
                          <span
                            class="repo-badge-label"
                            style="background: {badgeBg(
                              badge.hue,
                              darkMode.value
                            )}; color: {badgeFg(badge.hue, darkMode.value)};"
                          >
                            <RepoLabel githubRepo={project.githubRepo} subpath={project.subpath} />
                          </span>
                        {:else}
                          <span class="repo-line">
                            <RepoLabel githubRepo={project.githubRepo} subpath={project.subpath} />
                          </span>
                        {/if}
                      {:else}
                        No repo attached
                      {/if}
                    </div>
                  </button>
                </ContextMenu.Trigger>
                <ContextMenu.Content class="min-w-[172px]">
                  <ContextMenu.Item
                    disabled={status.kind === 'deleting'}
                    onSelect={() => projectActions.markProjectUnread(project)}
                  >
                    <Mail size={14} /> Mark as Unread
                  </ContextMenu.Item>
                  <ProjectStatusSubmenu {project} disabled={status.kind === 'deleting'} />
                  <ContextMenu.Separator />
                  <ContextMenu.Item
                    variant="destructive"
                    disabled={status.kind === 'deleting'}
                    onSelect={() => projectActions.requestRemoveProject(project)}
                  >
                    <Trash2 size={14} /> Remove Project
                  </ContextMenu.Item>
                </ContextMenu.Content>
              </ContextMenu.Root>
            </div>
          {/each}
        </div>
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
    margin-bottom: 14px;
  }

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

  .project-card:disabled {
    cursor: not-allowed;
  }

  .project-card.deleting {
    opacity: 0.75;
  }

  .project-card.deleting:hover {
    background: var(--bg-primary);
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

  .repo {
    margin-top: auto;
    font-size: var(--size-sm);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 4px;
    overflow: hidden;
  }

  .repo-line {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    gap: 5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-badge-label {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    padding: 1px 5px;
    border-radius: 4px;
    font-weight: 600;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .repo-badge-label :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .repo-badge-label :global(.repo-label-emphasis) {
    color: inherit;
  }

  .deleting-pill {
    width: fit-content;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border-muted);
    background-color: var(--bg-elevated);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
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

  .status-indicator {
    position: absolute;
    top: 10px;
    right: 10px;
    z-index: 5;
  }

  .status-indicator.spinner {
    color: var(--ui-accent);
  }

  .status-indicator.wave {
    color: var(--ui-accent);
  }

  .status-indicator.wave-spinner {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--ui-accent);
  }

  .status-indicator.unread-dot {
    width: 8px;
    height: 8px;
    background-color: var(--ui-accent);
    border-radius: 50%;
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
