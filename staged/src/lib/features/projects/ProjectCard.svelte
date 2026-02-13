<!--
  ProjectCard.svelte — Project entry on the landing page

  Flat title matching the project detail page style, with a list of recent
  branches underneath. Clicking the title navigates to the project detail.
  Clicking a branch navigates and scrolls to that branch's card.
-->
<script lang="ts">
  import { GitBranch, Cloud, ChevronRight } from 'lucide-svelte';
  import type { Project, Branch } from '../../types';
  import { projectDisplayName } from '../../shared/utils';
  import { selectProject, selectProjectAndBranch } from '../../navigation.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
  }

  let { project, branches }: Props = $props();

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
</script>

<div class="project-entry">
  <button class="project-header" onclick={() => selectProject(project.id)}>
    <span class="project-name">{projectDisplayName(project)}</span>
    <span class="meta-item">
      <GitBranch size={12} />
      {branches.length}
      {branches.length === 1 ? 'branch' : 'branches'}
    </span>
    <span class="chevron"><ChevronRight size={14} /></span>
  </button>

  {#if branches.length > 0}
    <div class="branches-list">
      {#each branches as branch (branch.id)}
        <button
          class="branch-row"
          onclick={() => selectProjectAndBranch(project.id, branch.id)}
          title={branch.branchName}
        >
          <span class="branch-icon">
            {#if branch.branchType === 'remote'}
              <span class="cloud-icon" class:running={branch.workspaceStatus === 'running'}
                ><Cloud size={12} /></span
              >
            {:else}
              <GitBranch size={12} />
            {/if}
          </span>
          <span class="branch-name">{branch.branchName}</span>
          <span class="branch-time">{formatRelativeTime(Math.floor(branch.updatedAt / 1000))}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .project-entry {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-left: -8px;
    margin-right: -8px;
  }

  .project-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    background-color: transparent;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition:
      background-color 0.15s ease,
      color 0.1s;
  }

  .project-header:hover {
    background-color: var(--bg-hover);
  }

  .project-name {
    font-size: var(--size-xl);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .meta-item {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .chevron {
    display: flex;
    align-items: center;
    margin-left: auto;
    color: var(--text-faint);
    opacity: 0;
    transition: opacity 0.15s ease;
  }

  .project-header:hover .chevron {
    opacity: 1;
  }

  .branches-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .branch-row {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 4px 8px;
    background-color: transparent;
    border: none;
    border-radius: 5px;
    font-size: var(--size-sm);
    color: var(--text-muted);
    text-align: left;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .branch-row:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .branch-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    color: var(--branch-color);
    opacity: 0.7;
  }

  .branch-row:hover .branch-icon {
    opacity: 1;
  }

  .cloud-icon {
    display: flex;
    align-items: center;
    color: var(--text-faint);
  }

  .cloud-icon.running {
    color: var(--text-accent);
  }

  .branch-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1;
  }

  .branch-time {
    flex-shrink: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }
</style>
