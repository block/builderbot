<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { quintIn } from 'svelte/easing';
  import { fade } from 'svelte/transition';
  import House from '@lucide/svelte/icons/house';
  import Plus from '@lucide/svelte/icons/plus';
  import Cloud from '@lucide/svelte/icons/cloud';
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
  import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import Sprout from '@lucide/svelte/icons/sprout';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import Mail from '@lucide/svelte/icons/mail';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import type { Project, ProjectRepo, Branch, WorkspaceStatus, RepoHomeItem } from '../../types';
  import { goHome, navigation, selectProject, showAllRepos } from '../layout/navigation.svelte';
  import {
    projectDisplayName,
    aggregateProjectPrStatus,
    projectHasCodeChanges,
    projectSubtitle,
    projectActivity,
  } from '../../shared/utils';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import { getProjectStatus } from './projectStatus';
  import {
    hydrateProjectsSidebarState,
    projectsSidebarState,
    setProjectsSidebarWidth,
    SIDEBAR_DEFAULT_WIDTH,
    SIDEBAR_MAX_WIDTH,
    SIDEBAR_MIN_WIDTH,
  } from './projectsSidebarState.svelte';
  import { viewport, watchViewport } from '../../shared/viewport.svelte';
  import SidebarPinnedRepo from './SidebarPinnedRepo.svelte';
  import * as commands from '../../api/commands';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import { reposUiEnabled } from '../../featureFlags';
  import { Button } from '$lib/components/ui/button';

  const devBranch = import.meta.env.VITE_DEV_BRANCH as string | undefined;

  interface Props {
    projects: Project[];
    loading?: boolean;
    error?: string | null;
    deletingProjectNames?: Map<string, string>;
    repoCountsByProject?: Map<string, number>;
    reposByProject?: Map<string, ProjectRepo[]>;
    showAllProjectsRow?: boolean;
    projectBranches?: Map<string, Branch[]>;
    onMarkProjectUnread?: (project: Project) => void;
    onRemoveProject?: (project: Project) => void | Promise<void>;
  }

  let {
    projects,
    loading = false,
    error = null,
    deletingProjectNames = new Map(),
    repoCountsByProject = new Map(),
    reposByProject = new Map(),
    showAllProjectsRow = true,
    projectBranches = new Map(),
    onMarkProjectUnread,
    onRemoveProject,
  }: Props = $props();

  let lastNavigationKey = `${navigation.activeView}:${navigation.selectedProjectId ?? ''}`;

  // ── Pinned repos ──
  let pinnedRepos = $state<RepoHomeItem[]>([]);
  let dragSourceIndex = $state<number | null>(null);

  async function loadPinnedRepos() {
    try {
      const all = await commands.listReposForHome();
      pinnedRepos = all.filter((r) => r.pinned);
    } catch (e) {
      console.error('[ProjectsSidebar] Failed to load pinned repos:', e);
    }
  }

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
        await loadPinnedRepos();
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
    if (active) node.scrollIntoView({ block: 'nearest' });
    return {
      update(active: boolean) {
        if (active) node.scrollIntoView({ block: 'nearest' });
      },
    };
  }

  function repoCountForProject(project: Project): number {
    return repoCountsByProject.get(project.id) ?? (project.githubRepo ? 1 : 0);
  }

  function getProjectPrStatus(
    projectId: string
  ): 'merged' | 'open' | 'closed' | 'checks_failing' | 'conflict' | null {
    const branches = projectBranches.get(projectId) || [];
    return aggregateProjectPrStatus(branches);
  }

  function getProjectWorkspaceStatus(projectId: string): WorkspaceStatus | null {
    const branches = projectBranches.get(projectId) || [];
    return branches.find((b) => b.workspaceStatus)?.workspaceStatus ?? null;
  }

  function cloudStatusClass(status: WorkspaceStatus | null): string {
    switch (status) {
      case 'running':
        return 'cloud-running';
      case 'starting':
        return 'cloud-starting';
      case 'error':
        return 'cloud-error';
      case 'stopped':
      case 'suspended':
      default:
        return 'cloud-inactive';
    }
  }

  let resizing = $state(false);
  let resizeStartX = 0;
  let resizeStartWidth = SIDEBAR_DEFAULT_WIDTH;
  let sidebarVisible = $derived(
    projectsSidebarState.hasProjects && !viewport.isMobile && !projectsSidebarState.collapsed
  );
  let sidebarStyle = $derived(`width: ${projectsSidebarState.width}px;`);

  onMount(() => {
    const stopWatchingViewport = watchViewport();
    void hydrateProjectsSidebarState();

    const onPinnedChanged = () => {
      void loadPinnedRepos();
    };
    if (reposUiEnabled) {
      void loadPinnedRepos();
      window.addEventListener('staged:pinned-repos-changed', onPinnedChanged);
    }

    return () => {
      stopWatchingViewport();
      if (reposUiEnabled) {
        window.removeEventListener('staged:pinned-repos-changed', onPinnedChanged);
      }
    };
  });

  onDestroy(() => {
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
    const nextNavigationKey = `${navigation.activeView}:${navigation.selectedProjectId ?? ''}`;
    if (nextNavigationKey !== lastNavigationKey) {
      lastNavigationKey = nextNavigationKey;
    }
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

    <div class="sidebar-body">
      {#if loading}
        <div class="state">Loading projects…</div>
      {:else if error}
        <div class="state error">{error}</div>
      {:else}
        <div class="projects-list">
          {#if reposUiEnabled && pinnedRepos.length > 0}
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

            <div class="pinned-repos-list" role="list" aria-label="Pinned repos">
              {#each pinnedRepos as repo, index (repo.githubRepo + '\t' + repo.subpath)}
                <SidebarPinnedRepo
                  {repo}
                  onReorderStart={handleDragStart(index)}
                  onReorderOver={handleDragOver(index)}
                  onReorderDrop={handleDrop(index)}
                  onReorderEnd={handleDragEnd()}
                  onPinnedReposChanged={loadPinnedRepos}
                />
              {/each}
            </div>

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
            {#each projects as project (project.id)}
              {@const status = getProjectStatus(
                project.id,
                deletingProjectNames,
                projectBranches.get(project.id) || []
              )}
              {@const repoCount = repoCountForProject(project)}
              {@const prStatus = getProjectPrStatus(project.id)}
              {@const workspaceStatus =
                project.location === 'remote' ? getProjectWorkspaceStatus(project.id) : null}
              {@const sessionTypes = projectStateStore.getRunningSessionTypes(project.id)}
              {@const repos = reposByProject.get(project.id) ?? []}
              {@const badges = repos
                .map((r) => repoBadgeStore.lookup(r.githubRepo, r.subpath))
                .filter((b): b is NonNullable<typeof b> => Boolean(b))
                .sort((a, b) => a.shortName.localeCompare(b.shortName))}
              {@const activity = projectActivity(sessionTypes, status.runActionPhase)}
              <ContextMenu.Root>
                <ContextMenu.Trigger
                  class="contents"
                  disabled={status.kind === 'deleting' || !onMarkProjectUnread || !onRemoveProject}
                >
                  <button
                    class="project-row project-item"
                    use:scrollIfActive={navigation.selectedProjectId === project.id}
                    class:active={navigation.selectedProjectId === project.id}
                    class:deleting={status.kind === 'deleting'}
                    onclick={() => openProject(project.id)}
                    disabled={status.kind === 'deleting'}
                    title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
                  >
                    <div class="row-main">
                      {#if project.location === 'remote'}
                        <Cloud size={14} class={cloudStatusClass(workspaceStatus)} />
                      {:else if prStatus === 'merged'}
                        <GitPullRequest size={14} class="pr-status-merged" />
                      {:else if prStatus === 'checks_failing'}
                        <GitPullRequest size={14} class="pr-status-checks-failing" />
                      {:else if prStatus === 'open'}
                        <GitPullRequest size={14} />
                      {:else if prStatus === 'closed'}
                        <GitPullRequestClosed size={14} />
                      {:else if prStatus === 'conflict'}
                        <GitPullRequestClosed size={14} class="pr-status-conflict" />
                      {:else if projectHasCodeChanges(projectBranches.get(project.id) || [])}
                        <GitPullRequestDraft size={14} class="pr-status-draft" />
                      {:else}
                        <Sprout size={14} class="pr-status-clean" />
                      {/if}
                      <div class="row-text">
                        <span class="project-name">{projectDisplayName(project)}</span>
                        <div class="row-meta">
                          {#if badges.length > 0}
                            <span class="badge-row">
                              {#each badges as badge}
                                <RepoBadge shortName={badge.shortName} hue={badge.hue} small />
                              {/each}
                            </span>
                            {#if activity}
                              <span class="activity-separator">&middot;</span>
                              <span class="activity-text">{activity}</span>
                            {/if}
                          {:else}
                            <span class="repo-count"
                              >{projectSubtitle(
                                repoCount,
                                sessionTypes,
                                status.runActionPhase
                              )}</span
                            >
                          {/if}
                        </div>
                      </div>
                    </div>
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
                {#if onMarkProjectUnread && onRemoveProject}
                  <ContextMenu.Content class="min-w-[172px]">
                    <ContextMenu.Item
                      disabled={status.kind === 'deleting'}
                      onSelect={() => onMarkProjectUnread!(project)}
                    >
                      <Mail size={14} /> Mark as Unread
                    </ContextMenu.Item>
                    <ContextMenu.Item
                      variant="destructive"
                      disabled={status.kind === 'deleting'}
                      onSelect={() => onRemoveProject!(project)}
                    >
                      <Trash2 size={14} /> Remove Project
                    </ContextMenu.Item>
                  </ContextMenu.Content>
                {/if}
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
  }

  .project-row.active .row-meta,
  .project-row.active .repo-count,
  .project-row.active :global(svg) {
    color: var(--text-primary);
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

  .row-main :global(svg.pr-status-merged) {
    stroke: var(--ui-success);
  }

  .row-main :global(svg.pr-status-conflict) {
    stroke: var(--ui-danger);
  }

  .row-main :global(svg.pr-status-checks-failing) {
    stroke: var(--ui-danger);
  }

  .row-main :global(svg.pr-status-draft) {
    stroke: var(--text-faint);
  }

  .row-main :global(svg.pr-status-clean) {
    stroke: var(--text-faint);
  }

  .row-main :global(svg.cloud-running) {
    stroke: var(--ui-accent);
  }

  .row-main :global(svg.cloud-starting) {
    stroke: var(--ui-info);
  }

  .row-main :global(svg.cloud-error) {
    stroke: var(--ui-danger);
  }

  .row-main :global(svg.cloud-inactive) {
    stroke: var(--text-muted);
  }

  .row-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .project-name {
    font-size: var(--size-sm);
    font-weight: 600;
    color: inherit;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 14px;
    font-size: calc(var(--size-xs) - 1px);
    line-height: 14px;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .repo-count {
    color: var(--text-faint);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge-row {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    min-width: 0;
    max-width: 100%;
    min-height: 14px;
    overflow: hidden;
    white-space: nowrap;
  }

  .activity-separator {
    color: var(--text-faint);
    margin: 0 1px;
  }

  .activity-text {
    color: var(--text-faint);
    min-width: 0;
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
    gap: 3px;
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
