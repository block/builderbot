<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { quintIn } from 'svelte/easing';
  import { fade } from 'svelte/transition';
  import Plus from '@lucide/svelte/icons/plus';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import Mail from '@lucide/svelte/icons/mail';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import {
    followWhileVisible,
    goHome,
    navigation,
    selectProject,
    showAllRepos,
  } from '../layout/navigation.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import { getProjectStatus } from './projectStatus';
  import { workspaceLifecycle } from './workspaceLifecycle.svelte';
  import {
    hydrateProjectsSidebarState,
    projectsSidebarState,
    setProjectsSidebarScrollTop,
    setProjectsSidebarWidth,
    SIDEBAR_DEFAULT_WIDTH,
    SIDEBAR_MAX_WIDTH,
    SIDEBAR_MIN_WIDTH,
  } from './projectsSidebarState.svelte';
  import { viewport, watchViewport } from '../../shared/viewport.svelte';
  import PinnedRepoCards from './PinnedRepoCards.svelte';
  import ProjectRowContent from './ProjectRowContent.svelte';
  import ProjectStatusSubmenu from './ProjectStatusSubmenu.svelte';
  import SidebarFilterMenu from './SidebarFilterMenu.svelte';
  import SidebarSearchField from './SidebarSearchField.svelte';
  import SidebarSectionHead from './SidebarSectionHead.svelte';
  import { projectFiltersStore } from './projectFilters.svelte';
  import {
    matchingProjectIds,
    matchingRepoKeys,
    resultsLabel,
    sidebarPinnedRepos,
  } from './sidebarSearch';
  import { projectsListViewState } from './projectsListViewState.svelte';
  import { projectActions } from './projectActions.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';

  const devBranch = import.meta.env.VITE_DEV_BRANCH as string | undefined;

  // All rendered data comes from the shared projectsData store; UI state
  // (width, scroll) lives here or in projectsSidebarState, and the search
  // query in projectsListViewState so the landing-page grid shares it.
  let projects = $derived(projectsDataStore.projects);
  let projectBranches = $derived(projectsDataStore.branchesByProject);
  let deletingProjectNames = $derived(projectsDataStore.deletingProjectNames);
  let loading = $derived(projectsDataStore.loading || !projectsDataStore.loaded);
  let error = $derived(projectsDataStore.error);

  let sidebarBodyEl = $state<HTMLDivElement | null>(null);
  let activeProjectRowEl = $state<HTMLElement | null>(null);
  let sidebarScrollRestored = $state(false);
  let restoreInProgress = false;
  let restoreToken = 0;
  let trackedSidebarBodyEl: HTMLDivElement | null = null;

  // ── Search ──
  // One field narrows both segments. The query is the landing-page grid's
  // (projectsListViewState.searchQuery): typing in either field narrows both
  // surfaces, so the sidebar list and the grid never disagree about which
  // projects are showing.
  //
  // Both segments render their whole list and the query only hides what it
  // rules out (see sidebarSearch.ts): rendering the narrowed lists rebuilt
  // every row and repo card a keystroke filtered out and back in.
  //
  // Followed key by key only while this layer is on screen, like the grid's
  // (see ProjectsList). Under home, the query changing is the grid's search
  // being typed into. Following each keystroke there re-hid rows and cards
  // nobody could see, so the sidebar catches up once typing pauses instead.
  const searchQuery = followWhileVisible(() => projectsListViewState.searchQuery);
  let searching = $derived(searchQuery.current.trim().length > 0);

  // ── Repos segment ──
  // The pinned shelf, narrowed by the query. Unpinned repos are reached via
  // All repos, never surfaced here.
  let pinnedRepos = $derived(sidebarPinnedRepos(projectsDataStore.homeRepos));
  let matchingRepos = $derived(matchingRepoKeys(pinnedRepos, searchQuery.current));
  let visibleRepoCount = $derived(matchingRepos?.size ?? pinnedRepos.length);

  // The view this sidebar sits beside is the workspace's project layer, not
  // necessarily the route on screen: the layer stays mounted under home, where
  // `selectedProjectId` is null, and the rows must keep highlighting the view
  // the user comes back to.
  let layerProjectId = $derived(
    navigation.projectLayer?.kind === 'project' ? navigation.projectLayer.projectId : null
  );
  let layerShowsRepos = $derived(navigation.projectLayer?.kind === 'repos');

  // Any repo (pinned or not) earns the repos section and its All repos link;
  // keeping it while the repos view is active means the link highlighting
  // that view can't vanish out from under it.
  let showReposSection = $derived(projectsDataStore.homeRepos.length > 0 || layerShowsRepos);

  // Project rows follow the shared filter state, with one exception: the
  // selected project stays visible (appended if filtered out) so the row
  // highlighting the active view can't vanish out from under it — the same
  // rule that keeps the repos section above. The search query is applied
  // last and is not exempted: a typed query is a deliberate narrowing (the
  // landing page's visibleProjects makes the same call).
  let sidebarProjects = $derived.by(() => {
    const filtered = projectFiltersStore.filteredProjects;
    const selectedId = layerProjectId;
    if (selectedId && !filtered.some((p) => p.id === selectedId)) {
      const selected = projects.find((p) => p.id === selectedId);
      if (selected) return [...filtered, selected];
    }
    return filtered;
  });
  let matchingProjects = $derived(matchingProjectIds(sidebarProjects, searchQuery.current));
  let visibleProjectCount = $derived(matchingProjects?.size ?? sidebarProjects.length);

  // Keep run-action state hydrated for the row status dots and the Running
  // filter — on the repos route this is the only mounted surface that can
  // feed branch data to the store (ProjectsList and ProjectHome run the same
  // sweep on their routes). The store dedupes branches it has already
  // queried, so overlapping with ProjectHome on the project route is cheap.
  $effect(() => {
    projectRunActionsStore
      .hydrateFromProjectBranches(projectsDataStore.branchesByProject)
      .catch(console.error);
  });

  function openProject(projectId: string) {
    const status = getProjectStatus(
      projectId,
      deletingProjectNames,
      projectBranches.get(projectId) || []
    );
    if (status.kind === 'deleting') return;
    selectProject(projectId);
  }

  function openAllProjects() {
    goHome();
  }

  function openNewProject() {
    window.dispatchEvent(new CustomEvent('staged:new-project'));
  }

  function scrollIfActive(node: HTMLElement, active: boolean) {
    let currentActive = active;
    if (active) {
      activeProjectRowEl = node;
    }
    return {
      update(nextActive: boolean) {
        if (currentActive === nextActive) return;
        currentActive = nextActive;
        if (nextActive) {
          activeProjectRowEl = node;
          if (sidebarScrollRestored) {
            node.scrollIntoView({ block: 'nearest' });
          }
        } else if (activeProjectRowEl === node) {
          activeProjectRowEl = null;
        }
      },
      destroy() {
        if (activeProjectRowEl === node) {
          activeProjectRowEl = null;
        }
      },
    };
  }

  function handleSidebarScroll() {
    if (!sidebarBodyEl) return;
    setProjectsSidebarScrollTop(sidebarBodyEl.scrollTop);
  }

  function saveSidebarScrollTopFromNode(node: HTMLDivElement) {
    const scrollTop = node.scrollTop;
    const canTrustScrollTop = scrollTop > 0 || node.clientHeight > 0 || node.scrollHeight > 0;

    if (!canTrustScrollTop) {
      return;
    }

    setProjectsSidebarScrollTop(scrollTop);
  }

  function trackSidebarBody(node: HTMLDivElement) {
    sidebarBodyEl = node;
    trackedSidebarBodyEl = node;
    sidebarScrollRestored = false;

    return {
      destroy() {
        saveSidebarScrollTopFromNode(node);
        if (sidebarBodyEl === node) {
          sidebarBodyEl = null;
          sidebarScrollRestored = false;
        }
        if (trackedSidebarBodyEl === node) {
          trackedSidebarBodyEl = null;
        }
      },
    };
  }

  function isFullyVisibleInSidebar(node: HTMLElement): boolean {
    if (!sidebarBodyEl) return true;

    const sidebarRect = sidebarBodyEl.getBoundingClientRect();
    const nodeRect = node.getBoundingClientRect();
    return nodeRect.top >= sidebarRect.top && nodeRect.bottom <= sidebarRect.bottom;
  }

  async function restoreSidebarScrollPosition() {
    if (!sidebarBodyEl || restoreInProgress) return;

    restoreInProgress = true;
    sidebarScrollRestored = false;
    const token = ++restoreToken;
    const requestedScrollTop = projectsSidebarState.scrollTop;

    try {
      await tick();
      if (token !== restoreToken || !sidebarBodyEl) {
        return;
      }

      sidebarBodyEl.scrollTop = requestedScrollTop;
      await tick();
      if (token !== restoreToken || !sidebarBodyEl) {
        return;
      }

      const activeRowVisible = activeProjectRowEl
        ? isFullyVisibleInSidebar(activeProjectRowEl)
        : null;

      if (activeProjectRowEl && !activeRowVisible) {
        activeProjectRowEl.scrollIntoView({ block: 'nearest' });
      }
      setProjectsSidebarScrollTop(sidebarBodyEl.scrollTop);
    } finally {
      if (token === restoreToken) {
        restoreInProgress = false;
        sidebarScrollRestored = true;
      }
    }
  }

  let resizing = $state(false);
  let resizeStartX = 0;
  let resizeStartWidth = SIDEBAR_DEFAULT_WIDTH;
  // Keep the sidebar up until a completed load proves there are no projects,
  // so it doesn't flash out during startup.
  let sidebarVisible = $derived(
    (projects.length > 0 || !projectsDataStore.loaded) && !viewport.isMobile
  );
  let sidebarStyle = $derived(`width: ${projectsSidebarState.width}px;`);

  onMount(() => {
    const stopWatchingViewport = watchViewport();
    void hydrateProjectsSidebarState();

    // Pin changes propagate through the store's repos-changed listener;
    // this mount only has to make sure the cache is warm.
    void projectsDataStore.ensureHomeReposLoaded();

    return () => {
      stopWatchingViewport();
    };
  });

  onDestroy(() => {
    if (trackedSidebarBodyEl) {
      saveSidebarScrollTopFromNode(trackedSidebarBodyEl);
    }
    stopResize();
  });

  function startResize(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    resizing = true;
    resizeStartX = e.clientX;
    resizeStartWidth = projectsSidebarState.width;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    window.addEventListener('pointermove', handleResizeMove);
    window.addEventListener('pointerup', stopResize);
  }

  function handleResizeMove(e: PointerEvent) {
    if (!resizing) return;
    const deltaX = e.clientX - resizeStartX;
    setProjectsSidebarWidth(resizeStartWidth + deltaX, false);
  }

  function stopResize() {
    if (!resizing) return;
    resizing = false;
    setProjectsSidebarWidth(projectsSidebarState.width, true);
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    window.removeEventListener('pointermove', handleResizeMove);
    window.removeEventListener('pointerup', stopResize);
  }

  function spring(t: number): number {
    const decay = 12;
    const frequency = 2;
    return 1 - Math.exp(-decay * t) * Math.cos(frequency * Math.PI * t);
  }

  function slideOpen(_node: HTMLElement) {
    const w = projectsSidebarState.width;
    return {
      duration: 550,
      easing: spring,
      css: (t: number) => `margin-left: ${(t - 1) * w}px`,
    };
  }

  function slideClose(_node: HTMLElement) {
    const w = projectsSidebarState.width;
    return {
      duration: 350,
      easing: quintIn,
      css: (t: number) => `margin-left: ${(t - 1) * w}px`,
    };
  }

  function handleResizeHandleKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      setProjectsSidebarWidth(projectsSidebarState.width - 16);
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      setProjectsSidebarWidth(projectsSidebarState.width + 16);
    } else if (e.key === 'Home') {
      e.preventDefault();
      setProjectsSidebarWidth(SIDEBAR_MIN_WIDTH);
    } else if (e.key === 'End') {
      e.preventDefault();
      setProjectsSidebarWidth(SIDEBAR_MAX_WIDTH);
    }
  }

  $effect(() => {
    const readyToRestore =
      sidebarVisible &&
      sidebarBodyEl &&
      !sidebarScrollRestored &&
      !restoreInProgress &&
      !loading &&
      !error;

    if (!readyToRestore) return;
    void restoreSidebarScrollPosition();
  });
</script>

{#if sidebarVisible}
  <aside class="projects-sidebar" class:resizing style={sidebarStyle} in:slideOpen out:slideClose>
    <div class="sidebar-header">
      <div class="title-row">
        <span class="brand-logo">
          <StagedIcon size={26} />
          <span class="brand-text">Staged</span>
          {#if devBranch}
            <span class="branch-badge">{devBranch}</span>
          {/if}
        </span>
      </div>
      <button
        type="button"
        class="new-project-button"
        title={viewport.showShortcutHints ? 'New project (⌘N)' : 'New project'}
        onclick={openNewProject}
      >
        <Plus size={14} strokeWidth={2.4} />
        <span class="new-project-label">New project</span>
      </button>
      <SidebarSearchField bind:value={projectsListViewState.searchQuery} />
    </div>

    <div class="sidebar-body" use:trackSidebarBody onscroll={handleSidebarScroll}>
      {#if error}
        <div class="state error">{error}</div>
      {:else if loading}
        <div class="state">Loading projects…</div>
      {:else}
        <div class="projects-list">
          {#if showReposSection}
            <SidebarSectionHead
              label={searching ? 'Repos' : 'Pinned repos'}
              count={searching ? resultsLabel(visibleRepoCount) : pinnedRepos.length}
            >
              {#snippet trailing()}
                <button
                  type="button"
                  class="all-repos-link"
                  class:active={layerShowsRepos}
                  onclick={showAllRepos}
                >
                  <FolderGit2 size={12} />
                  All repos
                  <ChevronRight size={11} />
                </button>
              {/snippet}
            </SidebarSectionHead>

            <!-- Mounted whenever anything is pinned, even while the query hides
                 every card, so clearing it doesn't rebuild them. -->
            {#if pinnedRepos.length > 0}
              <PinnedRepoCards
                repos={pinnedRepos}
                visibleKeys={matchingRepos}
                reorderable={!searching}
              />
            {/if}
            {#if searching && visibleRepoCount === 0}
              <div class="state no-matches">No repos match "{searchQuery.current.trim()}"</div>
            {:else if !searching && pinnedRepos.length === 0 && projectsDataStore.homeRepos.length > 0}
              <!-- Repos exist but none are pinned: point at where pinning lives
                   so the "Pinned repos 0" header isn't the only affordance.
                   With no repos at all (only reachable on the All repos view,
                   which keeps the section) the header stands alone — that
                   view is already the place to add one. -->
              <div class="state no-matches">
                <span>
                  Pin repos from
                  <button type="button" class="state-link" onclick={showAllRepos}>All repos</button>
                  to keep them here.
                </span>
              </div>
            {/if}
          {/if}

          {#if projects.length === 0}
            <div class="state">No projects yet.</div>
          {:else}
            <SidebarSectionHead
              label={searching ? 'Projects' : 'All projects'}
              count={searching ? resultsLabel(visibleProjectCount) : visibleProjectCount}
              active={layerProjectId === null && !layerShowsRepos}
              onclick={openAllProjects}
            >
              {#snippet trailing()}
                <SidebarFilterMenu />
              {/snippet}
            </SidebarSectionHead>
            {#if visibleProjectCount === 0}
              <div class="state no-matches">
                {#if searching}
                  <span>No projects match "{searchQuery.current.trim()}"</span>
                {:else}
                  <span>No projects match filters</span>
                  <button class="state-link" onclick={() => projectFiltersStore.clearFilters()}
                    >Clear filters</button
                  >
                {/if}
              </div>
            {/if}
            {#each sidebarProjects as project (project.id)}
              {@const status = getProjectStatus(
                project.id,
                deletingProjectNames,
                projectBranches.get(project.id) || [],
                workspaceLifecycle.getWorktreeErrors()
              )}
              {@const busy = status.kind === 'running' || status.kind === 'backgroundHold'}
              {@const busyTitle =
                status.kind === 'backgroundHold' ? 'Waiting on background work' : undefined}
              <ContextMenu.Root>
                <!-- Hidden here, on the display: contents wrapper, not on the
                     row inside it: WebKit re-shows children of a contents
                     element superlinearly (100 rows ~550 ms vs ~27 ms). -->
                <ContextMenu.Trigger
                  class="contents"
                  disabled={status.kind === 'deleting'}
                  hidden={matchingProjects !== null && !matchingProjects.has(project.id)}
                >
                  <button
                    class="project-row project-item"
                    use:scrollIfActive={layerProjectId === project.id}
                    class:active={layerProjectId === project.id}
                    class:deleting={status.kind === 'deleting'}
                    onclick={() => openProject(project.id)}
                    disabled={status.kind === 'deleting'}
                    title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
                  >
                    <ProjectRowContent {project} />
                    <div class="row-status">
                      {#if status.runActionPhase === 'running' && busy}
                        <span
                          class="status-running"
                          title={busyTitle}
                          in:fade={{ duration: 300, delay: 150 }}
                          out:fade={{ duration: 150 }}
                        >
                          <SineWave size={12} />
                          <Spinner size={12} />
                        </span>
                      {:else if status.runActionPhase === 'running'}
                        <span
                          class="status-running"
                          in:fade={{ duration: 300, delay: 150 }}
                          out:fade={{ duration: 150 }}
                        >
                          <SineWave size={12} />
                        </span>
                      {:else if status.kind === 'runAction' || busy}
                        <span
                          class="status-running"
                          title={busyTitle}
                          in:fade={{ duration: 300, delay: 150 }}
                          out:fade={{ duration: 150 }}
                        >
                          <Spinner size={12} />
                        </span>
                      {:else if status.kind === 'unread'}
                        <span
                          class="status-unread-dot"
                          aria-label="Unread updates"
                          in:fade={{ duration: 300, delay: 150 }}
                          out:fade={{ duration: 150 }}
                        ></span>
                      {:else if status.kind === 'deleting'}
                        <span
                          class="status-deleting"
                          in:fade={{ duration: 300, delay: 150 }}
                          out:fade={{ duration: 150 }}>Deleting…</span
                        >
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
            {/each}
          {/if}
        </div>
      {/if}
    </div>

    <button
      type="button"
      class="resize-handle"
      class:active={resizing}
      aria-label="Resize projects sidebar"
      onpointerdown={startResize}
      onkeydown={handleResizeHandleKeydown}
    ></button>
  </aside>
{/if}

<style>
  .projects-sidebar {
    --projects-sidebar-hover-bg: color-mix(in srgb, var(--text-primary) 4%, transparent);

    position: relative;
    flex-shrink: 0;
    border-right: 1px solid color-mix(in srgb, var(--border-subtle) 50%, transparent);
    background-color: var(--bg-app-bar);
    display: flex;
    flex-direction: column;
    min-height: 0;
    transition: width 0.14s ease;
  }

  .projects-sidebar.resizing {
    transition: none;
  }

  .sidebar-header {
    padding: 14px 12px 6px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .new-project-button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    width: 100%;
    height: 30px;
    border: none;
    border-radius: 999px;
    background: var(--ui-accent);
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 600;
    letter-spacing: 0.01em;
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      transform 0.1s ease;
  }

  .new-project-button:hover {
    background: var(--ui-accent-hover);
  }

  .new-project-button:active {
    transform: scale(0.985);
  }

  .new-project-button:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 2px;
  }

  .new-project-label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .brand-logo {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .brand-text {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: var(--size-lg);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.02em;
  }

  .branch-badge {
    font-size: 10px;
    font-weight: 500;
    color: var(--text-tertiary);
    background: var(--bg-elevated);
    padding: 1px 6px;
    border-radius: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 120px;
    min-width: 0;
  }

  .project-row:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: -1px;
  }

  .sidebar-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .projects-list {
    --project-row-bleed: 8px;

    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px var(--project-row-bleed);
  }

  /* The search hides project rows by their `display: contents` context-menu
     trigger. The `.contents` utility outranks the UA's [hidden] rule, so
     restate display: none here rather than lean on Tailwind preflight's
     `[hidden] { display: none !important }`. Matched by the slot the shadcn
     wrapper sets, not the class string passed to it. */
  .projects-list :global([data-slot='context-menu-trigger'][hidden]) {
    display: none;
  }

  .project-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: calc(100% + (2 * var(--project-row-bleed)));
    margin: 0 calc(-1 * var(--project-row-bleed));
    border: none;
    border-radius: 0;
    background-color: transparent;
    color: var(--text-primary);
    cursor: pointer;
    padding: 8px 10px;
    text-align: left;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .project-row:hover {
    color: var(--text-primary);
    background-color: var(--projects-sidebar-hover-bg);
  }

  .project-row.active {
    color: var(--text-primary);
    background-color: var(--bg-hover);
    /* Read by ProjectRowContent: brighten its meta line to match the row. */
    --project-row-meta-color: var(--text-primary);
  }

  .project-row.active .row-status :global(svg) {
    stroke: var(--text-primary);
  }

  .project-row.deleting {
    opacity: 0.7;
    cursor: not-allowed;
  }

  .project-row:disabled:hover {
    color: var(--text-muted);
    background-color: transparent;
  }

  .project-row.project-item {
    min-height: 52px;
  }

  .row-status {
    flex-shrink: 0;
    min-width: 18px;
    margin-top: 1px;
    display: flex;
    justify-content: flex-end;
    align-items: center;
  }

  .status-running {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--ui-accent);
  }

  .status-unread-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background-color: var(--ui-accent);
  }

  .status-deleting {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
    font-weight: 600;
  }

  .all-repos-link {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    padding: 3px 4px 3px 6px;
    cursor: pointer;
    white-space: nowrap;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .all-repos-link :global(svg) {
    stroke: var(--text-secondary);
  }

  .all-repos-link:hover,
  .all-repos-link.active {
    background: var(--projects-sidebar-hover-bg);
    color: var(--text-primary);
  }

  .all-repos-link:hover :global(svg),
  .all-repos-link.active :global(svg) {
    stroke: var(--text-primary);
  }

  .state {
    color: var(--text-muted);
    font-size: var(--size-xs);
    padding: 12px 10px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  .state.no-matches {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 2px;
  }

  .state-link {
    border: none;
    background: transparent;
    padding: 0;
    color: var(--ui-accent);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
  }

  .state-link:hover {
    text-decoration: underline;
  }

  .resize-handle {
    position: absolute;
    top: 0;
    right: -3px;
    width: 6px;
    height: 100%;
    cursor: col-resize;
    z-index: 5;
    border: none;
    background: transparent;
    padding: 0;
  }

  .resize-handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 2px;
    width: 2px;
    background-color: transparent;
    transition: background-color 0.15s ease;
  }

  .resize-handle:hover::after,
  .resize-handle:focus-visible::after,
  .resize-handle.active::after {
    background-color: var(--border-emphasis);
  }
</style>
