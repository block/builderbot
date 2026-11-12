<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import { quintIn } from 'svelte/easing';
  import { fade } from 'svelte/transition';
  import Plus from '@lucide/svelte/icons/plus';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import Mail from '@lucide/svelte/icons/mail';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import type { Project } from '../../types';
  import {
    followWhileVisible,
    goHome,
    navigation,
    selectBoard,
    selectProject,
    showAllRepos,
  } from '../layout/navigation.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import { disconnectedSessionsStore } from '../../stores/disconnectedSessions.svelte';
  import { disconnectedLabel } from '../../shared/disconnectedSession';
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
  import ProjectTagMenuItems from './ProjectTagMenuItems.svelte';
  import ProjectTransferMenuItem from './ProjectTransferMenuItem.svelte';
  import SidebarBoardRows from './SidebarBoardRows.svelte';
  import { transferringTitle } from './projectTransfer';
  import SidebarFilterMenu from './SidebarFilterMenu.svelte';
  import SidebarSearchField from './SidebarSearchField.svelte';
  import SidebarSectionHead from './SidebarSectionHead.svelte';
  import { projectFiltersStore } from './projectFilters.svelte';
  import {
    matchingRepoKeys,
    resultsLabel,
    sidebarPinnedRepos,
    sidebarProjectMatches,
  } from './sidebarSearch';
  import { searchOrderHold } from './projectSearch';
  import { projectSearchSnapshots } from './projectSearchContext';
  import { projectSearchIndex } from './projectSearchIndex.svelte';
  import { searchRowOrder } from './searchRowOrder.svelte';
  import { queryTerms } from '../../shared/textSearch';
  import { projectsListViewState } from './projectsListViewState.svelte';
  import { projectActions } from './projectActions.svelte';
  import { neighbourAfterRemove } from './focusAfterRemove';
  import { createScrollIfActive } from './activeRowScroll';
  import * as ContextMenu from '$lib/components/ui/context-menu';

  const devBranch = import.meta.env.VITE_DEV_BRANCH as string | undefined;

  // All rendered data comes from the shared projectsData store; UI state
  // (width, scroll) lives here or in projectsSidebarState, and the search
  // query in projectsListViewState so the landing-page grid shares it. Rows
  // come from liveProjects (here and through projectFiltersStore), so a
  // project's row goes as soon as its delete is confirmed.
  let projects = $derived(projectsDataStore.liveProjects);
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
  // The pinned board the layer shows, if any; its row highlights like a
  // project's. The rows themselves come from the tag catalog (SidebarBoardRows).
  let layerBoardGroupId = $derived(
    navigation.projectLayer?.kind === 'board' ? navigation.projectLayer.groupId : null
  );

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
  // Matched on branches, PRs, repo labels and titles too (projectSearch),
  // prepared once per data change and only while searching, like the grid;
  // rows found that way say why on their meta line. Ranked like the grid,
  // too, and like it re-ranked only when the query or the chips are edited
  // (searchOrderHold).
  const searchSnapshot = projectSearchSnapshots();
  let searchData = $derived(searching ? searchSnapshot() : null);
  const holdOrder = searchOrderHold();
  let projectMatches = $derived(
    holdOrder(
      searchQuery.current,
      projectFiltersStore.activeFilters,
      sidebarProjectMatches(sidebarProjects, searchQuery.current, searchData)
    )
  );
  // Null while the query is blank: every row shows, unordered.
  let projectRank = $derived(projectMatches.rank);
  // The order the {#each} renders the rows in: per keystroke they stay put
  // and CSS `order` moves them; once the query settles they are re-keyed
  // into rank order, so tab and reading order match the screen.
  const rowOrder = searchRowOrder(
    () => sidebarProjects,
    () => projectRank
  );
  let visibleProjectCount = $derived(projectRank?.size ?? sidebarProjects.length);
  let searchTerms = $derived(queryTerms(searchQuery.current));
  let matchReasons = $derived(projectMatches.reasons);

  // Branches are what a search reaches beyond names, and they only hydrate
  // per project on demand or via the store's idle drip. Pull the rest
  // forward once a search starts so it covers every project; the store
  // dedupes the ones already fetched.
  $effect(() => {
    if (!searching || !projectsDataStore.loaded) return;
    void projectsDataStore.projects;
    untrack(() => void projectsDataStore.ensureProjectsHydrated());
  });

  // Titles (notes, project notes, reviews) come from the lazily loaded search
  // index: fetch it once a query starts, if focusing the field didn't already.
  // Asked again on every keystroke so a load that failed mid-search retries
  // rather than leaving titles unmatched; writes during the search wait for
  // the index's coalesced refetch instead (projectSearchIndex).
  $effect(() => {
    if (!searchQuery.current.trim()) return;
    untrack(() => projectSearchIndex.ensureLoaded());
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

  function openProject(projectId: string) {
    const status = getProjectStatus(
      projectId,
      deletingProjectNames,
      projectBranches.get(projectId) || []
    );
    if (status.kind === 'deleting') return;
    selectProject(projectId);
  }

  // The row goes as soon as the remove is confirmed, taking focus with it, so
  // focus moves on to the row that takes its place. Removing the open
  // project opens the next one in the unfiltered list, and its row is the
  // one focus takes, even when the filter or search shows a different
  // neighbour. The list keeps that row as the selected one when the filter
  // rules it out, so highlight and focus agree.
  // While searching, the screen order is the rank's, not sidebarProjects'.
  function removeProject(project: Project) {
    const rank = projectRank;
    const shownIds =
      rank === null
        ? sidebarProjects.map((p) => p.id)
        : [...rank].sort(([, a], [, b]) => a - b).map(([id]) => id);
    const focusAfterRemove = neighbourAfterRemove(shownIds, project.id, (projectId) =>
      projectRank === null || projectRank.has(projectId)
        ? [...(sidebarBodyEl?.querySelectorAll<HTMLElement>('.project-row') ?? [])].find(
            (row) => row.dataset.projectId === projectId
          )
        : null
    );
    void projectActions.requestRemoveProject(project, focusAfterRemove);
  }

  function openAllProjects() {
    goHome();
  }

  function openNewProject() {
    window.dispatchEvent(new CustomEvent('staged:new-project'));
  }

  const scrollIfActive = createScrollIfActive({
    activeRow: () => activeProjectRowEl,
    setActiveRow: (node) => {
      activeProjectRowEl = node;
    },
    scrollRestored: () => sidebarScrollRestored,
  });

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
      <SidebarSearchField
        bind:value={projectsListViewState.searchQuery}
        onfocus={() => projectSearchIndex.ensureLoaded()}
        onkeydown={rowOrder.fieldKeydown}
      />
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
              active={layerProjectId === null && !layerShowsRepos && layerBoardGroupId === null}
              onclick={openAllProjects}
            >
              {#snippet trailing()}
                <SidebarFilterMenu />
              {/snippet}
            </SidebarSectionHead>
            <!-- The pinned boards, between the heading and the rows. A search
                 lists project matches only, so they step aside while one is
                 typed, like the heading's label changes. -->
            {#if !searching}
              <SidebarBoardRows activeGroupId={layerBoardGroupId} onSelect={selectBoard} />
            {/if}
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
            <!-- The rows' own flex column, so the search's `order` ranks them
                 among themselves and never past the section headers. -->
            <div class="project-rows" onpointerdowncapture={rowOrder.rowsPointerdown}>
              {#each rowOrder.rows as project (project.id)}
                {@const status = getProjectStatus(
                  project.id,
                  deletingProjectNames,
                  projectBranches.get(project.id) || [],
                  workspaceLifecycle.getWorktreeErrors(),
                  projectsDataStore.transferringProjects
                )}
                {@const busy =
                  status.kind === 'running' ||
                  status.kind === 'backgroundHold' ||
                  status.kind === 'transferring'}
                {@const busyTitle =
                  status.kind === 'backgroundHold'
                    ? 'Waiting on background work'
                    : status.kind === 'transferring'
                      ? transferringTitle(projectsDataStore.transferringProjects.get(project.id))
                      : undefined}
                <ContextMenu.Root>
                  <!-- Hidden here, on the display: contents wrapper, not on the
                       row inside it: WebKit re-shows children of a contents
                       element superlinearly (100 rows ~550 ms vs ~27 ms). -->
                  <ContextMenu.Trigger
                    class="contents"
                    hidden={projectRank !== null && !projectRank.has(project.id)}
                  >
                    <!-- The flex item, the trigger being display: contents, so
                         the rank's `order` goes here. -->
                    <button
                      class="project-row project-item"
                      style:order={rowOrder.orderOf(project.id)}
                      data-project-id={project.id}
                      use:scrollIfActive={layerProjectId === project.id}
                      class:active={layerProjectId === project.id}
                      onclick={() => openProject(project.id)}
                    >
                      <ProjectRowContent
                        {project}
                        matchReason={matchReasons.get(project.id)}
                        matchTerms={searchTerms}
                      />
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
                        {:else if status.kind === 'disconnected'}
                          {@const label = disconnectedLabel(
                            disconnectedSessionsStore.projectReconcileState(project.id)
                          )}
                          <span
                            class="status-disconnected"
                            title={label}
                            aria-label={label}
                            in:fade={{ duration: 300, delay: 150 }}
                            out:fade={{ duration: 150 }}
                          >
                            <CloudOff size={12} />
                          </span>
                        {:else if status.kind === 'unread'}
                          <span
                            class="status-unread-dot"
                            aria-label="Unread updates"
                            in:fade={{ duration: 300, delay: 150 }}
                            out:fade={{ duration: 150 }}
                          ></span>
                        {/if}
                      </div>
                    </button>
                  </ContextMenu.Trigger>
                  <ContextMenu.Content class="min-w-[172px]">
                    <ContextMenu.Item onSelect={() => projectActions.markProjectUnread(project)}>
                      <Mail size={14} /> Mark as Unread
                    </ContextMenu.Item>
                    <ProjectTagMenuItems {project} />
                    <ContextMenu.Separator />
                    <ProjectTransferMenuItem {project} />
                    <ContextMenu.Item
                      variant="destructive"
                      disabled={status.kind === 'transferring'}
                      onSelect={() => removeProject(project)}
                    >
                      <Trash2 size={14} /> Remove Project
                    </ContextMenu.Item>
                  </ContextMenu.Content>
                </ContextMenu.Root>
              {/each}
            </div>
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
    --projects-sidebar-field-bg: color-mix(in srgb, var(--text-primary) 5%, transparent);
    --projects-sidebar-field-bg-hover: color-mix(in srgb, var(--text-primary) 7%, transparent);

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
    /* Clear of the home indicator on a landscape phone (App.svelte's main
       leaves the bottom inset to the panes' content). */
    padding-bottom: calc(10px + env(safe-area-inset-bottom));
  }

  .project-rows {
    display: flex;
    flex-direction: column;
    gap: 4px;
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

  .status-disconnected {
    display: inline-flex;
    align-items: center;
    color: var(--ui-warning);
  }

  .status-unread-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background-color: var(--ui-accent);
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
