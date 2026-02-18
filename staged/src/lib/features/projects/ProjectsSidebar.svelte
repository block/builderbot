<script lang="ts">
  import { FolderGit2, House, Plus } from 'lucide-svelte';
  import type { Project } from '../../types';
  import { goHome, navigation, selectProject } from '../../navigation.svelte';
  import { projectDisplayName } from '../../shared/utils';
  import Spinner from '../../shared/Spinner.svelte';
  import { getProjectStatus } from './projectStatus';

  interface Props {
    projects: Project[];
    loading?: boolean;
    error?: string | null;
    deletingProjectNames?: Map<string, string>;
    repoCountsByProject?: Map<string, number>;
    showAllProjectsRow?: boolean;
  }

  let {
    projects,
    loading = false,
    error = null,
    deletingProjectNames = new Map(),
    repoCountsByProject = new Map(),
    showAllProjectsRow = true,
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
</script>

<aside class="projects-sidebar">
  <div class="sidebar-header">
    <div class="title-row">
      <h2>Projects</h2>
      <span class="count">{projects.length}</span>
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
            <button
              class="project-row"
              class:active={navigation.selectedProjectId === project.id}
              class:deleting={status.kind === 'deleting'}
              onclick={() => openProject(project.id)}
              disabled={status.kind === 'deleting'}
              title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
            >
              <div class="row-main">
                <FolderGit2 size={14} />
                <div class="row-text">
                  <span class="project-name">{projectDisplayName(project)}</span>
                  <div class="row-meta">
                    <span class="repo-count">{repoCount} {repoCount === 1 ? 'repo' : 'repos'}</span>
                  </div>
                </div>
              </div>
              <div class="row-status">
                {#if status.kind === 'running'}
                  <span class="status-running">
                    <Spinner size={12} />
                    <span class="running-count">{status.runningCount}</span>
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
          <Plus size={14} />
          New project
        </button>
      </div>
    {/if}
  </div>
</aside>

<style>
  .projects-sidebar {
    width: 280px;
    flex-shrink: 0;
    border-right: 1px solid var(--border-muted);
    background-color: var(--bg-surface);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .sidebar-header {
    padding: 14px 12px 10px;
    border-bottom: 1px solid var(--border-muted);
    display: flex;
    flex-direction: column;
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  h2 {
    margin: 0;
    font-size: var(--size-sm);
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: 0.01em;
    text-transform: uppercase;
  }

  .count {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    padding: 1px 7px;
  }

  .new-project-button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    border: 1px dashed var(--border-muted);
    border-radius: 8px;
    background-color: transparent;
    color: var(--text-muted);
    padding: 8px 10px;
    font-size: var(--size-xs);
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .new-project-button:hover {
    border-color: var(--ui-accent);
    color: var(--text-primary);
    background-color: var(--bg-hover);
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

  .list-new-project-button {
    margin-top: 8px;
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
    background-color: var(--bg-hover);
    border-color: var(--border-muted);
  }

  .project-row.active {
    color: var(--text-primary);
    background-color: var(--bg-elevated);
    border-color: var(--border-emphasis);
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
    gap: 8px;
    flex: 1;
    min-width: 0;
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
    gap: 6px;
    color: var(--ui-accent);
  }

  .running-count {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: calc(var(--size-xs) - 1px);
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    padding: 0 5px;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    line-height: 1.35;
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
</style>
