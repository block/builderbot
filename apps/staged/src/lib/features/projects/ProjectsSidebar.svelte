<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { quintIn } from 'svelte/easing';
  import { fade } from 'svelte/transition';
  import House from '@lucide/svelte/icons/house';
  import Plus from '@lucide/svelte/icons/plus';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import Mail from '@lucide/svelte/icons/mail';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import type { RepoHomeItem } from '../../types';
  import { goHome, navigation, selectProject, showAllRepos } from '../layout/navigation.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import { getProjectStatus } from './projectStatus';
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
  import RepoCard from './RepoCard.svelte';
  import ProjectRowContent from './ProjectRowContent.svelte';
  import SidebarFilterRow from './SidebarFilterRow.svelte';
  import { projectFiltersStore } from './projectFilters.svelte';
  import * as commands from '../../api/commands';
  import { projectActions } from './projectActions.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import { Button } from '$lib/components/ui/button';

  const devBranch = import.meta.env.VITE_DEV_BRANCH as string | undefined;

  interface Props {
    showAllProjectsRow?: boolean;
  }

  let { showAllProjectsRow = true }: Props = $props();

  // All rendered data comes from the shared projectsData store; only UI
  // state (width, scroll, drag) lives here or in projectsSidebarState.
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

  // ── Pinned repos ──
  // Synced from the shared home-repos cache; kept as local state so a drag
  // reorder applies optimistically before the persisted order round-trips.
  let pinnedRepos = $state<RepoHomeItem[]>([]);
  let dragSourceIndex = $state<number | null>(null);

  $effect(() => {
    pinnedRepos = projectsDataStore.homeRepos.filter((r) => r.pinned);
  });

  // Any repo (pinned or not) earns the All Repos entry; keeping it while the
  // repos view is active means the row highlighting that view can't vanish
  // out from under it.
  let showAllReposRow = $derived(
    projectsDataStore.homeRepos.length > 0 || navigation.showReposList
  );

  // Project rows follow the shared filter state, with one exception: the
  // selected project stays visible (appended if filtered out) so the row
  // highlighting the active view can't vanish out from under it — the same
  // rule that keeps the All Repos row above.
  let sidebarProjects = $derived.by(() => {
    const filtered = projectFiltersStore.filteredProjects;
    const selectedId = navigation.selectedProjectId;
    if (!selectedId || filtered.some((p) => p.id === selectedId)) return filtered;
    const selected = projects.find((p) => p.id === selectedId);
    return selected ? [...filtered, selected] : filtered;
  });

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

  function handleDragStart(index: number) {
    return (e: DragEvent) => {
      dragSourceIndex = index;
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = 'move';
      }
    };
  }

  function handleDragOver(index: number) {
    return (e: DragEvent) => {
      e.preventDefault();
      if (e.dataTransfer) {
        e.dataTransfer.dropEffect = 'move';
      }
    };
  }

  function handleDrop(index: number) {
    return async (_e: DragEvent) => {
      if (dragSourceIndex === null || dragSourceIndex === index) {
        dragSourceIndex = null;
        return;
      }

      // Reorder the array
      const items = [...pinnedRepos];
      const [moved] = items.splice(dragSourceIndex, 1);
      items.splice(index, 0, moved);
      pinnedRepos = items;
      dragSourceIndex = null;

      // Persist the new order
      const orderedKeys: [string, string][] = items.map((r) => [r.githubRepo, r.subpath]);
      try {
        await commands.reorderPinnedRepos(orderedKeys);
      } catch (e) {
        console.error('[ProjectsSidebar] Failed to reorder pinned repos:', e);
        // Reload to get the correct order
        await projectsDataStore.refreshHomeRepos();
      }
    };
  }

  function handleDragEnd() {
    return () => {
      dragSourceIndex = null;
    };
  }

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
    </div>

    <div class="sidebar-body" use:trackSidebarBody onscroll={handleSidebarScroll}>
      {#if error}
        <div class="state error">{error}</div>
      {:else if loading}
        <div class="state">Loading projects…</div>
      {:else}
        <div class="projects-list">
          {#if showAllReposRow}
            <button
              class="project-row all-repos-row"
              class:active={navigation.showReposList}
              onclick={showAllRepos}
            >
              <div class="row-main">
                <FolderGit2 size={14} />
                <span class="project-name">All Repos</span>
              </div>
            </button>

            {#if pinnedRepos.length > 0}
              <div class="pinned-repos-list" role="list" aria-label="Pinned repos">
                {#each pinnedRepos as repo, index (repo.githubRepo + '\t' + repo.subpath)}
                  <RepoCard
                    {repo}
                    hidePinButton
                    reorderable
                    onReorderStart={handleDragStart(index)}
                    onReorderOver={handleDragOver(index)}
                    onReorderDrop={handleDrop(index)}
                    onReorderEnd={handleDragEnd()}
                    onChange={() => projectsDataStore.refreshHomeRepos()}
                  />
                {/each}
              </div>
            {/if}

            <div class="section-divider"></div>
          {/if}

          {#if showAllProjectsRow}
            <button
              class="project-row all-projects-row"
              class:active={navigation.selectedProjectId === null && !navigation.showReposList}
              onclick={openAllProjects}
            >
              <div class="row-main">
                <House size={14} />
                <span class="project-name">All Projects</span>
              </div>
            </button>
          {/if}

          {#if projects.length === 0}
            <div class="state">No projects yet.</div>
          {:else}
            <SidebarFilterRow />
            {#if sidebarProjects.length === 0}
              <div class="state no-matches">
                <span>No projects match filters</span>
                <button
                  class="clear-filters-link"
                  onclick={() => projectFiltersStore.clearFilters()}>Clear filters</button
                >
              </div>
            {/if}
            {#each sidebarProjects as project (project.id)}
              {@const status = getProjectStatus(
                project.id,
                deletingProjectNames,
                projectBranches.get(project.id) || []
              )}
              <ContextMenu.Root>
                <ContextMenu.Trigger class="contents" disabled={status.kind === 'deleting'}>
                  <button
                    class="project-row project-item"
                    use:scrollIfActive={navigation.selectedProjectId === project.id}
                    class:active={navigation.selectedProjectId === project.id}
                    class:deleting={status.kind === 'deleting'}
                    onclick={() => openProject(project.id)}
                    disabled={status.kind === 'deleting'}
                    title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
                  >
                    <ProjectRowContent {project} />
                    <div class="row-status">
                      {#if status.runActionPhase === 'running' && status.kind === 'running'}
                        <span
                          class="status-running"
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
                      {:else if status.kind === 'runAction' || status.kind === 'running'}
                        <span
                          class="status-running"
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
          <Button
            variant="ghost"
            class="group h-auto w-full justify-start gap-2.5 px-2.5 py-2 font-medium text-foreground hover:bg-[var(--projects-sidebar-hover-bg)] hover:text-foreground"
            title={viewport.showShortcutHints ? 'New project (⌘N)' : 'New project'}
            onclick={openNewProject}
          >
            <span
              class="flex size-4 shrink-0 items-center justify-center rounded-full bg-[var(--border-muted)] transition-colors group-hover:bg-[var(--border-emphasis)]"
            >
              <Plus size={12} />
            </span>
            New project
          </Button>
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
    padding: 14px 12px 10px;
    display: flex;
    flex-direction: column;
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

  .project-row.active .row-status :global(svg),
  .all-repos-row.active .row-main :global(svg) {
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

  .row-main {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    flex: 1;
    min-width: 0;
  }

  .row-main :global(svg) {
    flex-shrink: 0;
    width: 16px;
  }

  .project-name {
    font-size: var(--size-sm);
    font-weight: 600;
    color: inherit;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .all-repos-row :global(svg) {
    stroke: var(--text-secondary);
  }

  .pinned-repos-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 2px 0;
  }

  .section-divider {
    height: 1px;
    background: color-mix(in srgb, var(--border-subtle) 40%, transparent);
    margin: 4px 4px;
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

  .clear-filters-link {
    border: none;
    background: transparent;
    padding: 0;
    color: var(--ui-accent);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
  }

  .clear-filters-link:hover {
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
