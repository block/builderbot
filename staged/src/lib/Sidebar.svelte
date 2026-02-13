<!--
  Sidebar.svelte - Collapsible left sidebar with project navigation and activity feed

  Top section: clickable project list for navigation.
  Bottom section: cumulative, recency-sorted list of commits and notes.
  When a project is selected, the activity feed is filtered to that project.

  Also shows a "Builds" section when there are running
  (or recently finished) actions across any branch.
-->
<script lang="ts">
  import {
    Folder,
    GitCommitHorizontal,
    StickyNote,
    FolderTree,
    CheckCircle,
    AlertCircle,
    StopCircle,
  } from 'lucide-svelte';
  import { onMount, onDestroy } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import Spinner from './shared/Spinner.svelte';
  import type { Branch, CommitTimelineItem, NoteTimelineItem } from './types';
  import { projectStore } from './features/projects/projectStore.svelte';
  import { projectDisplayName } from './shared/utils';
  import { navigation, goHome } from './navigation.svelte';
  import * as commands from './commands';
  import {
    preferences,
    toggleSidebarGroupByProject,
    setSidebarWidth,
  } from './features/settings/preferences.svelte';
  import { type ActionStatusEvent, type ActionStatus } from './features/actions/actions';

  // ── Running builds ──

  type RunningBuild = {
    executionId: string;
    actionId: string;
    actionName: string;
    branchId: string;
    branchName: string;
    status: ActionStatus;
    startedAt: number;
    completedAt?: number | null;
  };

  let runningBuilds = $state<RunningBuild[]>([]);

  /** Ticks every second so elapsed times re-render while builds are running. */
  let now = $state(Date.now());

  // Tick the clock every second while there are running builds.
  $effect(() => {
    const hasRunning = runningBuilds.some((b) => b.status === 'running');
    if (!hasRunning) return;
    const id = setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => clearInterval(id);
  });

  /** Resolve a branchId to a human-readable branch name. */
  function branchNameFor(branchId: string): string {
    for (const project of projectStore.projects) {
      const branches = projectStore.branchesByProject.get(project.id) || [];
      const match = branches.find((b) => b.id === branchId);
      if (match) return match.branchName;
    }
    return branchId;
  }

  // Subscribe to action_status events from the backend (same pattern as BranchCard).
  let unlistenActionStatus: UnlistenFn | null = null;

  listen<ActionStatusEvent>('action_status', (event) => {
    const { executionId, branchId, actionId, actionName, status, startedAt, completedAt } =
      event.payload;

    console.log('[Sidebar] action_status event:', status, actionName, executionId);

    const existingIdx = runningBuilds.findIndex((b) => b.executionId === executionId);

    if (status === 'running') {
      if (existingIdx === -1) {
        runningBuilds.push({
          executionId,
          actionId,
          actionName,
          branchId,
          branchName: branchNameFor(branchId),
          status: 'running',
          startedAt: startedAt ?? Date.now(),
        });
        console.log('[Sidebar] Added running build, total:', runningBuilds.length);
      }
    } else {
      // completed / failed / stopped
      if (existingIdx !== -1) {
        runningBuilds[existingIdx].status = status;
        runningBuilds[existingIdx].completedAt = completedAt;

        // Auto-remove after a delay so the user can see the final status.
        const delay = status === 'completed' ? 3000 : 6000;
        const eid = executionId; // capture for closure
        setTimeout(() => {
          runningBuilds = runningBuilds.filter((b) => b.executionId !== eid);
        }, delay);
      }
    }
  }).then((fn) => {
    unlistenActionStatus = fn;
    console.log('[Sidebar] action_status listener registered');
  });

  onDestroy(() => {
    unlistenActionStatus?.();
  });

  /** Format elapsed or total duration for a build. */
  function formatBuildDuration(build: RunningBuild): string {
    const end = build.status === 'running' ? now : (build.completedAt ?? now);
    const elapsed = Math.max(0, Math.floor((end - build.startedAt) / 1000));
    if (elapsed < 60) return `${elapsed}s`;
    const mins = Math.floor(elapsed / 60);
    const secs = elapsed % 60;
    return `${mins}m ${secs.toString().padStart(2, '0')}s`;
  }

  function handleBuildClick(build: RunningBuild) {
    scrollToBranch(build.branchId);
    // Give the scroll a moment then request the output modal to open.
    setTimeout(() => {
      window.dispatchEvent(
        new CustomEvent('staged:show-action-output', {
          detail: {
            executionId: build.executionId,
            actionName: build.actionName,
            branchId: build.branchId,
          },
        })
      );
    }, 100);
  }

  // ── Unified timeline item for display ──

  type SidebarItem = {
    key: string;
    kind: 'commit' | 'note';
    title: string;
    /** Short SHA for commits, empty for notes. */
    meta: string;
    /** Unix seconds — used for sorting. */
    timestamp: number;
    branchId: string;
    branchName: string;
    projectId: string;
    projectName: string;
  };

  // Timeline data fetched for all branches
  let timelineItems = $state<SidebarItem[]>([]);
  let timelineLoading = $state(false);

  /** Strip XML-tagged context blocks from display text. */
  function stripXmlTags(text: string): string {
    return text.replace(/<(action|branch-history)>[\s\S]*?<\/\1>/g, '').trim();
  }

  // Track which set of branch IDs we've already fetched for, so we only
  // re-fetch when the branch list actually changes.
  let lastBranchKey = '';

  $effect(() => {
    // Build a stable key from the set of branch IDs
    const allBranches: Branch[] = [];
    for (const project of projectStore.projects) {
      const branches = projectStore.branchesByProject.get(project.id) || [];
      allBranches.push(...branches);
    }
    const branchKey = allBranches
      .map((b) => b.id)
      .sort()
      .join(',');

    if (branchKey === lastBranchKey || projectStore.loading) return;
    lastBranchKey = branchKey;

    if (allBranches.length === 0) {
      timelineItems = [];
      return;
    }

    fetchAllTimelines(allBranches);
  });

  async function fetchAllTimelines(allBranches: Branch[]) {
    timelineLoading = true;

    // Build a lookup for branch → project info
    const projectNameById = new Map<string, string>();
    for (const p of projectStore.projects) {
      projectNameById.set(p.id, projectDisplayName(p));
    }

    try {
      const items: SidebarItem[] = [];

      await Promise.all(
        allBranches.map(async (branch) => {
          try {
            const tl = await commands.getBranchTimeline(branch.id);
            const pName = projectNameById.get(branch.projectId) || '';

            for (const commit of tl.commits) {
              // Skip pending/failed commits (no SHA)
              if (!commit.sha) continue;
              items.push({
                key: `c-${commit.sha}`,
                kind: 'commit',
                title: stripXmlTags(commit.subject),
                meta: commit.shortSha,
                timestamp: commit.timestamp,
                branchId: branch.id,
                branchName: branch.branchName,
                projectId: branch.projectId,
                projectName: pName,
              });
            }

            for (const note of tl.notes) {
              // Skip generating/failed notes
              if (note.sessionStatus === 'running' || !note.content?.trim()) continue;
              items.push({
                key: `n-${note.id}`,
                kind: 'note',
                title: stripXmlTags(note.title),
                meta: '',
                timestamp: Math.floor(note.createdAt / 1000),
                branchId: branch.id,
                branchName: branch.branchName,
                projectId: branch.projectId,
                projectName: pName,
              });
            }
          } catch {
            // If a single branch fails, skip it silently
          }
        })
      );

      // Sort descending by timestamp (most recent first)
      items.sort((a, b) => b.timestamp - a.timestamp);
      timelineItems = items;
    } finally {
      timelineLoading = false;
    }
  }

  // ── Filtered view: when a project is selected, filter to that project ──

  let filteredTimelineItems = $derived(
    navigation.selectedProjectId
      ? timelineItems.filter((item) => item.projectId === navigation.selectedProjectId)
      : timelineItems
  );

  // ── Grouped view: items grouped by project, projects ordered by most recent item ──

  let groupedItems = $derived.by(() => {
    const groups = new Map<
      string,
      { projectId: string; projectName: string; items: SidebarItem[]; maxTs: number }
    >();

    for (const item of filteredTimelineItems) {
      let group = groups.get(item.projectId);
      if (!group) {
        group = {
          projectId: item.projectId,
          projectName: item.projectName,
          items: [],
          maxTs: 0,
        };
        groups.set(item.projectId, group);
      }
      group.items.push(item);
      if (item.timestamp > group.maxTs) group.maxTs = item.timestamp;
    }

    return [...groups.values()].sort((a, b) => b.maxTs - a.maxTs);
  });

  // ── Helpers ──

  function formatRelativeTime(timestamp: number): string {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m`;
    if (diffHours < 24) return `${diffHours}h`;
    if (diffDays < 7) return `${diffDays}d`;
    return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }

  function scrollToBranch(branchId: string) {
    window.dispatchEvent(new CustomEvent('staged:scroll-to-branch', { detail: { branchId } }));
  }

  // ── Resize handle logic ──

  let resizing = $state(false);

  function onResizeStart(e: MouseEvent) {
    e.preventDefault();
    resizing = true;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    const startX = e.clientX;
    const startWidth = preferences.sidebarWidth;

    function onMouseMove(ev: MouseEvent) {
      const newWidth = startWidth + (ev.clientX - startX);
      setSidebarWidth(newWidth);
    }

    function onMouseUp() {
      resizing = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
    }

    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
  }
</script>

<aside class="sidebar" class:resizing style:width="{preferences.sidebarWidth}px">
  <!-- ── Top nav ── -->
  <nav class="sidebar-nav">
    <button
      class="nav-item"
      class:active={!navigation.selectedProjectId}
      onclick={goHome}
      title="All projects"
    >
      <Folder size={14} />
      <span>Projects</span>
    </button>
  </nav>

  <!-- ── Activity feed ── -->
  <div class="activity-header">
    <span class="sidebar-title">Recents</span>
    <button
      class="group-toggle"
      class:active={preferences.sidebarGroupByProject}
      onclick={toggleSidebarGroupByProject}
      title={preferences.sidebarGroupByProject ? 'Show flat list' : 'Group by project'}
    >
      <FolderTree size={13} />
    </button>
  </div>

  <div class="sidebar-list">
    {#if runningBuilds.length > 0}
      <div class="builds-section">
        <div class="builds-header">
          <span class="sidebar-title">Builds</span>
        </div>
        {#each runningBuilds as build (build.executionId)}
          <button
            class="build-item"
            class:running={build.status === 'running'}
            class:completed={build.status === 'completed'}
            class:failed={build.status === 'failed' || build.status === 'stopped'}
            onclick={() => handleBuildClick(build)}
            title={`${build.actionName} on ${build.branchName}\nClick to view output`}
          >
            <span class="build-icon">
              {#if build.status === 'running'}
                <Spinner size={12} />
              {:else if build.status === 'completed'}
                <CheckCircle size={12} />
              {:else if build.status === 'failed'}
                <AlertCircle size={12} />
              {:else}
                <StopCircle size={12} />
              {/if}
            </span>
            <span class="item-content">
              <span class="item-title">{build.actionName}</span>
              <span class="item-meta">
                <span class="item-branch">{build.branchName}</span>
                <span class="item-time">{formatBuildDuration(build)}</span>
              </span>
            </span>
          </button>
        {/each}
      </div>
    {/if}

    {#if projectStore.loading || timelineLoading}
      <div class="sidebar-empty">Loading...</div>
    {:else if filteredTimelineItems.length === 0}
      <div class="sidebar-empty">No activity yet</div>
    {:else if preferences.sidebarGroupByProject}
      <!-- Grouped by project -->
      {#each groupedItems as group (group.projectId)}
        <div class="project-group">
          <div class="project-group-header">
            <span class="project-group-name">{group.projectName}</span>
          </div>
          {#each group.items as item (item.key)}
            <button
              class="timeline-item"
              onclick={() => scrollToBranch(item.branchId)}
              title={`${item.title}\n${item.branchName}`}
            >
              <span
                class="item-icon"
                class:commit={item.kind === 'commit'}
                class:note={item.kind === 'note'}
              >
                {#if item.kind === 'commit'}
                  <GitCommitHorizontal size={12} />
                {:else}
                  <StickyNote size={12} />
                {/if}
              </span>
              <span class="item-content">
                <span class="item-title">{item.title}</span>
                <span class="item-meta">
                  <span class="item-branch">{item.branchName}</span>
                  <span class="item-time">{formatRelativeTime(item.timestamp)}</span>
                </span>
              </span>
            </button>
          {/each}
        </div>
      {/each}
    {:else}
      <!-- Flat list sorted by recency -->
      {#each filteredTimelineItems as item (item.key)}
        <button
          class="timeline-item"
          onclick={() => scrollToBranch(item.branchId)}
          title={`${item.title}\n${item.branchName} — ${item.projectName}`}
        >
          <span
            class="item-icon"
            class:commit={item.kind === 'commit'}
            class:note={item.kind === 'note'}
          >
            {#if item.kind === 'commit'}
              <GitCommitHorizontal size={12} />
            {:else}
              <StickyNote size={12} />
            {/if}
          </span>
          <span class="item-content">
            <span class="item-title">{item.title}</span>
            <span class="item-meta">
              <span class="item-branch">{item.branchName}</span>
              <span class="item-time">{formatRelativeTime(item.timestamp)}</span>
            </span>
          </span>
        </button>
      {/each}
    {/if}
  </div>

  <!-- Resize handle -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="resize-handle" onmousedown={onResizeStart}></div>
</aside>

<style>
  .sidebar {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-deepest);
    border-right: 1px solid var(--border-subtle);
    overflow: hidden;
    position: relative;
  }

  /* Disable text selection and pointer events on content while dragging */
  .sidebar.resizing {
    user-select: none;
  }

  .resize-handle {
    position: absolute;
    top: 0;
    right: -3px;
    width: 6px;
    height: 100%;
    cursor: col-resize;
    z-index: 10;
  }

  .resize-handle::after {
    content: '';
    position: absolute;
    top: 0;
    left: 50%;
    width: 2px;
    height: 100%;
    transform: translateX(-50%);
    background-color: transparent;
    transition: background-color 0.15s;
  }

  .resize-handle:hover::after,
  .sidebar.resizing .resize-handle::after {
    background-color: var(--ui-accent);
  }

  /* ── Top nav ── */

  .sidebar-nav {
    display: flex;
    flex-direction: column;
    padding: 8px 6px 4px;
    flex-shrink: 0;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 6px;
    background: transparent;
    border: none;
    border-radius: 5px;
    color: var(--text-muted);
    font-size: var(--size-base);
    font-weight: 500;
    text-align: left;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .nav-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .nav-item.active {
    color: var(--text-primary);
  }

  .sidebar-title {
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  /* ── Activity header ── */

  .activity-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px 6px;
    flex-shrink: 0;
    border-top: 1px solid var(--border-subtle);
  }

  .group-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 3px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .group-toggle:hover {
    color: var(--text-muted);
    background-color: var(--bg-hover);
  }

  .group-toggle.active {
    color: var(--ui-accent);
  }

  .sidebar-list {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 0 6px 12px;
  }

  /* Thin scrollbar matching the app's scrollbar style */
  .sidebar-list::-webkit-scrollbar {
    width: 5px;
  }

  .sidebar-list::-webkit-scrollbar-track {
    background: transparent;
  }

  .sidebar-list::-webkit-scrollbar-thumb {
    background-color: var(--scrollbar-thumb);
    border-radius: 3px;
  }

  .sidebar-list::-webkit-scrollbar-thumb:hover {
    background-color: var(--scrollbar-thumb-hover);
  }

  .sidebar-empty {
    padding: 16px 8px;
    font-size: var(--size-base);
    color: var(--text-faint);
    text-align: center;
  }

  /* ── Builds section ── */

  .builds-section {
    padding-bottom: 6px;
    margin-bottom: 4px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .builds-header {
    padding: 0 6px 4px;
  }

  .build-item {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    width: 100%;
    padding: 5px 6px;
    background: transparent;
    border: none;
    border-radius: 5px;
    color: var(--text-muted);
    font-size: var(--size-base);
    text-align: left;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .build-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .build-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .build-item.running .build-icon {
    color: var(--ui-accent);
  }

  .build-item.completed .build-icon {
    color: var(--ui-accent);
  }

  .build-item.failed .build-icon {
    color: var(--ui-danger);
  }

  /* Project group header */
  .project-group {
    margin-bottom: 4px;
  }

  .project-group-header {
    padding: 8px 6px 4px;
  }

  .project-group-name {
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-muted);
    letter-spacing: -0.01em;
  }

  /* Timeline item */
  .timeline-item {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    width: 100%;
    padding: 5px 6px;
    background: transparent;
    border: none;
    border-radius: 5px;
    color: var(--text-muted);
    font-size: var(--size-base);
    text-align: left;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .timeline-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .item-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .item-icon.commit {
    color: var(--timeline-commit);
  }

  .item-icon.note {
    color: var(--timeline-note);
  }

  .timeline-item:hover .item-icon {
    opacity: 0.9;
  }

  .item-content {
    display: flex;
    flex-direction: column;
    min-width: 0;
    gap: 2px;
  }

  .item-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.3;
  }

  .item-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    color: var(--text-faint);
    line-height: 1.2;
  }

  .item-branch {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .item-time {
    flex-shrink: 0;
    opacity: 0.7;
  }
</style>
