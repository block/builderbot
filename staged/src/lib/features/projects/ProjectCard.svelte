<!--
  ProjectCard.svelte — Card for the projects landing page

  Shows project name, branch count, last activity timestamp, and a short
  list of recent commits. Clicking the card navigates to the project detail.
-->
<script lang="ts">
  import { Folder, GitBranch, GitCommitHorizontal } from 'lucide-svelte';
  import type { Project } from '../../types';
  import { projectDisplayName } from '../../shared/utils';
  import { selectProject } from '../../navigation.svelte';

  interface RecentCommit {
    shortSha: string;
    subject: string;
    relativeTime: string;
  }

  interface Props {
    project: Project;
    branchCount: number;
    lastActivity: string;
    recentCommits: RecentCommit[];
  }

  let { project, branchCount, lastActivity, recentCommits }: Props = $props();
</script>

<button class="project-card" onclick={() => selectProject(project.id)}>
  <div class="card-header">
    <div class="project-info">
      <span class="folder-icon"><Folder size={16} /></span>
      <span class="project-name">{projectDisplayName(project)}</span>
    </div>
    <span class="last-activity" title="Last activity">{lastActivity}</span>
  </div>

  <div class="card-meta">
    <span class="meta-item">
      <GitBranch size={12} />
      {branchCount}
      {branchCount === 1 ? 'branch' : 'branches'}
    </span>
  </div>

  {#if recentCommits.length > 0}
    <div class="commits-list">
      {#each recentCommits as commit}
        <div class="commit-row">
          <span class="commit-icon"><GitCommitHorizontal size={11} /></span>
          <span class="commit-subject">{commit.subject}</span>
          <span class="commit-time">{commit.relativeTime}</span>
        </div>
      {/each}
    </div>
  {:else}
    <div class="no-commits">No commits yet</div>
  {/if}
</button>

<style>
  .project-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 16px 18px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    cursor: pointer;
    text-align: left;
    transition:
      border-color 0.15s ease,
      background-color 0.15s ease;
  }

  .project-card:hover {
    border-color: var(--border-muted);
    background-color: var(--bg-hover);
  }

  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .project-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .folder-icon {
    display: flex;
    align-items: center;
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .project-name {
    font-size: var(--size-lg);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .last-activity {
    font-size: var(--size-xs);
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .card-meta {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .meta-item {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .commits-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-top: 4px;
    border-top: 1px solid var(--border-subtle);
  }

  .commit-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
  }

  .commit-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    color: var(--commit-color);
    opacity: 0.7;
  }

  .commit-subject {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1;
  }

  .commit-time {
    flex-shrink: 0;
    color: var(--text-faint);
    font-size: 10px;
  }

  .no-commits {
    font-size: var(--size-xs);
    color: var(--text-faint);
    padding-top: 4px;
    border-top: 1px solid var(--border-subtle);
  }
</style>
