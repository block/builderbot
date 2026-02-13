<!--
  Sidebar.svelte - Collapsible left sidebar showing all commits and notes

  Fetches timelines for every tracked branch and renders a cumulative,
  recency-sorted list of commits and notes across all projects.
  Supports a "group by project" toggle that organizes items under
  project headings. Clicking an item scrolls the main content to its
  parent branch card.
-->
<script lang="ts">
  import { GitCommitHorizontal, StickyNote, FolderTree } from 'lucide-svelte';
  import type { Branch, CommitTimelineItem, NoteTimelineItem } from './types';
  import { projectStore } from './features/projects/projectStore.svelte';
  import { projectDisplayName } from './shared/utils';
  import * as commands from './commands';
  import {
    preferences,
    toggleSidebarGroupByProject,
    setSidebarWidth,
  } from './features/settings/preferences.svelte';

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

  // ── Grouped view: items grouped by project, projects ordered by most recent item ──

  let groupedItems = $derived.by(() => {
    const groups = new Map<
      string,
      { projectId: string; projectName: string; items: SidebarItem[]; maxTs: number }
    >();

    for (const item of timelineItems) {
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
  <div class="sidebar-header">
    <span class="sidebar-title">Activity</span>
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
    {#if projectStore.loading || timelineLoading}
      <div class="sidebar-empty">Loading...</div>
    {:else if timelineItems.length === 0}
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
      {#each timelineItems as item (item.key)}
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

  .sidebar-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 12px 8px;
    flex-shrink: 0;
  }

  .sidebar-title {
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
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
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-align: center;
  }

  /* Project group header */
  .project-group {
    margin-bottom: 4px;
  }

  .project-group-header {
    padding: 8px 6px 4px;
  }

  .project-group-name {
    font-size: var(--size-xs);
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
    font-size: var(--size-xs);
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
    font-size: 10px;
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
