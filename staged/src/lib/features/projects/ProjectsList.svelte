<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { FolderGit2, Plus } from 'lucide-svelte';
  import type { Project } from '../../types';
  import * as commands from '../../commands';
  import { projectDisplayName } from '../../shared/utils';
  import { selectProject } from '../../navigation.svelte';
  import NewProjectModal from './NewProjectModal.svelte';

  let projects = $state<Project[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showNewProjectModal = $state(false);

  onMount(() => {
    loadProjects();

    const onNewProject = () => {
      showNewProjectModal = true;
    };
    window.addEventListener('staged:new-project', onNewProject);
    return () => window.removeEventListener('staged:new-project', onNewProject);
  });

  async function loadProjects() {
    loading = true;
    error = null;
    try {
      projects = await commands.listProjects();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function handleProjectCreated(project: Project) {
    if (!projects.some((p) => p.id === project.id)) {
      projects = [...projects, project];
    }
    showNewProjectModal = false;
  }
</script>

<div class="projects-list-page">
  <div class="content">
    <div class="header">
      <h1>Projects</h1>
      {#if !loading && projects.length > 0}
        <span class="count">{projects.length}</span>
      {/if}
    </div>

    {#if loading}
      <div class="state">Loading projects…</div>
    {:else if error}
      <div class="state error">{error}</div>
    {:else if projects.length === 0}
      <div class="empty-state">
        <p>No projects yet.</p>
        <button class="new-project-button" onclick={() => (showNewProjectModal = true)}>
          <Plus size={14} />
          Add Project
        </button>
      </div>
    {:else}
      <div class="projects-grid">
        {#each projects as project (project.id)}
          <button class="project-card" onclick={() => selectProject(project.id)}>
            <div class="card-header">
              <FolderGit2 size={16} />
              <span>{projectDisplayName(project)}</span>
            </div>
            <div class="repo">{project.githubRepo}</div>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if showNewProjectModal}
  <NewProjectModal
    onCreated={handleProjectCreated}
    onClose={() => (showNewProjectModal = false)}
    onDetecting={() => {}}
  />
{/if}

<style>
  .projects-list-page {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-chrome);
  }

  .content {
    flex: 1;
    overflow: auto;
    padding: 20px 24px 24px;
    max-width: 900px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
  }

  .header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 18px;
  }

  h1 {
    margin: 0;
    font-size: var(--size-2xl);
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: -0.02em;
  }

  .count {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    color: var(--text-muted);
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    padding: 2px 8px;
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  .empty-state {
    color: var(--text-muted);
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }

  .empty-state p {
    margin: 0;
  }

  .new-project-button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .new-project-button:hover {
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .projects-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 12px;
  }

  .project-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    text-align: left;
    background: var(--bg-surface);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    padding: 14px;
    color: inherit;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      background-color 0.15s ease,
      transform 0.15s ease;
  }

  .project-card:hover {
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
    transform: translateY(-1px);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
  }

  .repo {
    color: var(--text-muted);
    font-size: var(--size-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
