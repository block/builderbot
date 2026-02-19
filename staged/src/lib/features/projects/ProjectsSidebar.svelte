<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { quintIn } from 'svelte/easing';
  import {
    FolderGit2,
    House,
    Plus,
    GitPullRequest,
    GitPullRequestClosed,
    GitPullRequestDraft,
    GitBranch,
  } from 'lucide-svelte';
  import type { Project, Branch } from '../../types';
  import { goHome, navigation, selectProject } from '../../navigation.svelte';
  import { projectDisplayName, aggregateProjectPrStatus } from '../../shared/utils';
  import Spinner from '../../shared/Spinner.svelte';
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

  interface Props {
    projects: Project[];
    loading?: boolean;
    error?: string | null;
    deletingProjectNames?: Map<string, string>;
    repoCountsByProject?: Map<string, number>;
    showAllProjectsRow?: boolean;
    projectBranches?: Map<string, Branch[]>;
  }

  let {
    projects,
    loading = false,
    error = null,
    deletingProjectNames = new Map(),
    repoCountsByProject = new Map(),
    showAllProjectsRow = true,
    projectBranches = new Map(),
  }: Props = $props();

  function openProject(projectId: string) {
    const status = getProjectStatus(projectId, deletingProjectNames);
    if (status.kind === 'deleting') return;
    selectProject(projectId);
  }

  function openNewProject() {
    window.dispatchEvent(new CustomEvent('staged:new-project'));
  }

  function repoCountForProject(project: Project): number {
    return repoCountsByProject.get(project.id) ?? (project.githubRepo ? 1 : 0);
  }

  function getProjectPrStatus(projectId: string): 'merged' | 'open' | 'closed' | 'conflict' | null {
    const branches = projectBranches.get(projectId) || [];
    return aggregateProjectPrStatus(branches);
  }

  let resizing = $state(false);
  let resizeStartX = 0;
  let resizeStartWidth = SIDEBAR_DEFAULT_WIDTH;

  onMount(() => {
    void hydrateProjectsSidebarState();
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
</script>

{#if !projectsSidebarState.collapsed && projectsSidebarState.hasProjects}
  <aside
    class="projects-sidebar"
    class:resizing
    style={`width: ${projectsSidebarState.width}px;`}
    in:slideOpen
    out:slideClose
  >
    <div class="sidebar-header">
      <div class="title-row">
        <span class="brand-logo">
          <StagedIcon size={26} />
          <span class="brand-text">staged</span>
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
          {#if showAllProjectsRow}
            <button
              class="project-row all-projects-row"
              class:active={navigation.selectedProjectId === null}
              onclick={goHome}
              title="Show all projects"
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
              {@const status = getProjectStatus(project.id, deletingProjectNames)}
              {@const repoCount = repoCountForProject(project)}
              {@const prStatus = getProjectPrStatus(project.id)}
              <button
                class="project-row"
                class:active={navigation.selectedProjectId === project.id}
                class:deleting={status.kind === 'deleting'}
                onclick={() => openProject(project.id)}
                disabled={status.kind === 'deleting'}
                title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
              >
                <div class="row-main">
                  {#if prStatus === 'merged'}
                    <GitPullRequest size={14} class="pr-status-merged" />
                  {:else if prStatus === 'open'}
                    <GitPullRequest size={14} />
                  {:else if prStatus === 'closed'}
                    <GitPullRequestClosed size={14} />
                  {:else if prStatus === 'conflict'}
                    <GitPullRequestClosed size={14} class="pr-status-conflict" />
                  {:else}
                    <GitPullRequestDraft size={14} class="pr-status-draft" />
                  {/if}
                  <div class="row-text">
                    <span class="project-name">{projectDisplayName(project)}</span>
                    <div class="row-meta">
                      <span class="repo-count"
                        >{repoCount} {repoCount === 1 ? 'repo' : 'repos'}</span
                      >
                    </div>
                  </div>
                </div>
                <div class="row-status">
                  {#if status.kind === 'running'}
                    <span class="status-running">
                      <Spinner size={12} />
                    </span>
                  {:else if status.kind === 'unread'}
                    <span class="status-unread-dot" aria-label="Unread updates"></span>
                  {:else if status.kind === 'deleting'}
                    <span class="status-deleting">Deleting…</span>
                  {/if}
                </div>
              </button>
            {/each}
          {/if}
          <button
            class="new-project-button list-new-project-button"
            onclick={openNewProject}
            title="New project (⌘N)"
          >
            <span class="plus-icon"><Plus size={12} /></span>
            New project
          </button>
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
    position: relative;
    flex-shrink: 0;
    border-right: 1px solid var(--border-subtle);
    background-color: var(--bg-surface);
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

  .plus-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border: none;
    border-radius: 50%;
    background-color: var(--border-muted);
    flex-shrink: 0;
  }

  .new-project-button {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    border: none;
    border-radius: 8px;
    background-color: transparent;
    color: var(--text-muted);
    padding: 8px 10px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .new-project-button:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .new-project-button:hover .plus-icon {
    background-color: var(--border-emphasis);
  }

  .new-project-button:focus-visible,
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
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 8px;
  }

  .project-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    border: 1px solid transparent;
    border-radius: 8px;
    background-color: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 8px 10px;
    text-align: left;
    transition: all 0.15s ease;
  }

  .project-row:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
    border-color: var(--border-subtle);
  }

  .project-row.active {
    color: var(--text-primary);
    background-color: var(--bg-hover);
    border-color: var(--border-muted);
  }

  .project-row.deleting {
    opacity: 0.7;
    border-style: dashed;
    cursor: not-allowed;
  }

  .project-row:disabled:hover {
    color: var(--text-muted);
    background-color: transparent;
    border-color: transparent;
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

  .row-main :global(svg.pr-status-draft) {
    stroke: var(--text-faint);
  }

  .row-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
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
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
    white-space: nowrap;
  }

  .repo-count {
    color: var(--text-faint);
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
