<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { FolderGit2 } from 'lucide-svelte';
  import type { Project, Branch } from '../../types';
  import * as commands from '../../commands';
  import { projectDisplayName, aggregateProjectPrStatus } from '../../shared/utils';
  import { selectProject } from '../../navigation.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import ProjectsSidebar from './ProjectsSidebar.svelte';
  import { getProjectStatus } from './projectStatus';
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import Spinner from '../../shared/Spinner.svelte';

  let projects = $state<Project[]>([]);
  let projectBranches = $state<Map<string, Branch[]>>(new Map());
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showNewProjectModal = $state(false);
  let isCommandKeyHeld = $state(false);
  let deletingProjectNames = $state<Map<string, string>>(new Map());
  let repoCountsByProject = $state<Map<string, number>>(new Map());
  let repoCountLoadGeneration = 0;

  onMount(() => {
    loadProjects();

    const onNewProject = () => {
      showNewProjectModal = true;
    };
    const onProjectDeleteStart = (event: Event) => {
      const detail = (event as CustomEvent<{ projectId?: string; name?: string }>).detail;
      const projectId = detail?.projectId;
      if (!projectId) return;
      const name =
        detail?.name ?? projects.find((project) => project.id === projectId)?.name ?? 'Project';
      deletingProjectNames = new Map(deletingProjectNames).set(projectId, name);
    };
    const onProjectDeleteEnd = (event: Event) => {
      const detail = (event as CustomEvent<{ projectId?: string }>).detail;
      const projectId = detail?.projectId;
      if (!projectId) return;
      const next = new Map(deletingProjectNames);
      next.delete(projectId);
      deletingProjectNames = next;
      loadProjects();
    };
    window.addEventListener('staged:new-project', onNewProject);
    window.addEventListener('staged:project-delete-start', onProjectDeleteStart);
    window.addEventListener('staged:project-delete-end', onProjectDeleteEnd);
    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
      window.removeEventListener('staged:project-delete-start', onProjectDeleteStart);
      window.removeEventListener('staged:project-delete-end', onProjectDeleteEnd);
    };
  });

  async function loadProjects() {
    loading = true;
    error = null;
    try {
      const loadedProjects = await commands.listProjects();
      projects = loadedProjects;
      repoCountsByProject = new Map(
        loadedProjects.map((project) => [project.id, project.githubRepo ? 1 : 0] as const)
      );
      void hydrateRepoCounts(loadedProjects);
      // Load branches for each project to calculate PR status
      const branchesMap = new Map<string, Branch[]>();
      await Promise.all(
        loadedProjects.map(async (project) => {
          try {
            const branches = await commands.listBranchesForProject(project.id);
            branchesMap.set(project.id, branches);
          } catch (e) {
            console.error(`Failed to load branches for project ${project.id}:`, e);
            branchesMap.set(project.id, []);
          }
        })
      );
      projectBranches = branchesMap;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function hydrateRepoCounts(projectList: Project[]) {
    const generation = ++repoCountLoadGeneration;
    const counts = await Promise.all(
      projectList.map(async (project) => {
        try {
          const repos = await commands.listProjectRepos(project.id);
          return [project.id, repos.length] as const;
        } catch (e) {
          console.error(`[ProjectsList] Failed to load repo count for project '${project.id}':`, e);
          return [project.id, project.githubRepo ? 1 : 0] as const;
        }
      })
    );
    if (generation !== repoCountLoadGeneration) return;
    repoCountsByProject = new Map(counts);
  }

  function handleProjectCreated(project: Project) {
    if (!projects.some((p) => p.id === project.id)) {
      projects = [...projects, project];
    }
    repoCountsByProject = new Map(repoCountsByProject).set(project.id, project.githubRepo ? 1 : 0);
    void hydrateRepoCounts(projects);
    showNewProjectModal = false;
    selectProject(project.id);
  }

  function isProjectDeleting(projectId: string): boolean {
    return deletingProjectNames.has(projectId);
  }

  function openProject(projectId: string) {
    if (isProjectDeleting(projectId)) return;
    selectProject(projectId);
  }

  function getProjectPrStatus(
    projectId: string
  ): 'success' | 'warning' | 'error' | 'neutral' | 'pending' | null {
    const branches = projectBranches.get(projectId) || [];
    return aggregateProjectPrStatus(branches);
  }

  function verifyCommandKeyState(e: KeyboardEvent | MouseEvent) {
    // Verify the command key is actually held down by checking the event's metaKey/ctrlKey
    const actuallyHeld = e.metaKey || e.ctrlKey;
    if (isCommandKeyHeld && !actuallyHeld) {
      isCommandKeyHeld = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    if (isInput) return;

    // Track command key state
    if (e.metaKey || e.ctrlKey) {
      isCommandKeyHeld = true;
    } else {
      // Any non-command key press while we think command is held means it's not
      verifyCommandKeyState(e);
    }

    // Command+N to open new project modal
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'n') {
      e.preventDefault();
      showNewProjectModal = true;
      return;
    }

    // Command+1-9 to open projects by number
    if ((e.metaKey || e.ctrlKey) && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      const index = parseInt(e.key) - 1;
      if (index < projects.length) {
        openProject(projects[index].id);
      }
    }
  }

  function handleKeyup(e: KeyboardEvent) {
    // Verify the actual state first
    verifyCommandKeyState(e);
    if (!e.metaKey && !e.ctrlKey) {
      isCommandKeyHeld = false;
    }
  }

  function handleBlur() {
    // Reset command key state when window loses focus
    // This handles cases like Command+Tab where keyup isn't received
    isCommandKeyHeld = false;
  }

  function handleMouseMove(e: MouseEvent) {
    // Verify command key state on mouse movement
    // This catches cases where the state got stuck (e.g., Command+Tab back to app)
    if (isCommandKeyHeld) {
      verifyCommandKeyState(e);
    }
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  onkeyup={handleKeyup}
  onblur={handleBlur}
  onmousemove={handleMouseMove}
/>

<div class="projects-list-page">
  <ProjectsSidebar
    {projects}
    {loading}
    {error}
    {deletingProjectNames}
    {repoCountsByProject}
    showAllProjectsRow={true}
  />

  <div class="main-panel">
    <div class="content" class:empty-layout={!loading && !error && projects.length === 0}>
      {#if loading}
        <div class="state">Loading projects…</div>
      {:else if error}
        <div class="state error">{error}</div>
      {:else if projects.length === 0}
        <div class="empty-state">
          <div class="welcome-header">
            <StagedIcon size={28} />
            <h2>welcome to <span class="mono accent">staged</span></h2>
          </div>
          <p class="welcome-subtitle">
            Create your first project to get started
            <button
              class="kbd-btn"
              onclick={() => (showNewProjectModal = true)}
              title="New project"
            >
              +
            </button>
            <span class="shortcut-hint">(⌘N)</span>
          </p>
          <GitTreeAnimation />
        </div>
      {:else}
        <div class="projects-grid">
          <button
            class="project-card new-project-card"
            onclick={() => (showNewProjectModal = true)}
          >
            <div class="new-project-content">
              <span class="new-project-label">+ New project</span>
            </div>
          </button>
          {#each projects as project, index (project.id)}
            {@const status = getProjectStatus(project.id, deletingProjectNames)}
            {@const prStatus = getProjectPrStatus(project.id)}
            <button
              class="project-card"
              class:deleting={status.kind === 'deleting'}
              onclick={() => openProject(project.id)}
              disabled={status.kind === 'deleting'}
              title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
            >
              {#if isCommandKeyHeld && index < 9}
                <div class="keyboard-shortcut-overlay">
                  <span class="command-icon">⌘</span>
                  <span class="number">{index + 1}</span>
                </div>
              {/if}
              {#if status.kind === 'running'}
                <div class="status-indicator spinner">
                  <Spinner size={14} />
                </div>
              {:else if status.kind === 'unread'}
                <div class="status-indicator unread-dot"></div>
              {/if}
              <div class="card-header">
                <FolderGit2 size={16} class={prStatus ? `pr-status-${prStatus}` : ''} />
                <span>{projectDisplayName(project)}</span>
              </div>
              {#if status.kind === 'deleting'}
                <div class="deleting-pill" role="status" aria-live="polite">Deleting…</div>
              {/if}
              <div class="repo">{project.githubRepo ?? 'No repo attached'}</div>
              <div class="repo">
                {project.location === 'remote' ? 'Remote workspace' : 'Local worktrees'}
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

{#if showNewProjectModal}
  <NewProjectModal onCreated={handleProjectCreated} onClose={() => (showNewProjectModal = false)} />
{/if}

<style>
  .projects-list-page {
    --sidebar-title-offset: 42px;
    flex: 1;
    min-height: 0;
    display: flex;
    min-width: 0;
    background-color: var(--bg-chrome);
    overflow: hidden;
  }

  .main-panel {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .content {
    flex: 1;
    overflow: auto;
    padding: var(--sidebar-title-offset) 24px 24px;
    max-width: 900px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .content.empty-layout {
    max-width: none;
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 20px;
  }

  .welcome-header {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .welcome-header h2 {
    font-size: var(--size-xl);
    font-weight: 500;
    color: var(--text-primary);
    margin: 0;
  }

  .welcome-header .mono {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    letter-spacing: -0.02em;
  }

  .welcome-header .accent {
    color: var(--ui-accent);
  }

  .welcome-subtitle {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .welcome-subtitle .kbd-btn {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: var(--size-xs);
    padding: 1px 5px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--ui-accent);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  .welcome-subtitle .kbd-btn:hover {
    background-color: var(--bg-hover);
    border-color: var(--ui-accent);
  }

  .welcome-subtitle .shortcut-hint {
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .empty-state :global(.animation-wrapper) {
    width: min(1400px, calc(100vw - 48px));
  }

  .projects-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 12px;
  }

  .project-card {
    position: relative;
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

  .project-card:disabled {
    cursor: not-allowed;
    transform: none;
  }

  .project-card.deleting {
    border-style: dashed;
    opacity: 0.75;
  }

  .project-card.deleting:hover {
    border-color: var(--border-muted);
    background: var(--bg-surface);
    transform: none;
  }

  .new-project-card {
    border-style: dashed;
    color: var(--text-primary);
    align-items: center;
    justify-content: center;
  }

  .new-project-content {
    display: inline-flex;
    align-items: baseline;
    gap: 10px;
  }

  .new-project-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--ui-accent);
  }

  .new-project-card:hover {
    border-color: var(--ui-accent);
    background-color: var(--bg-elevated);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
    padding-right: 24px;
  }

  .repo {
    color: var(--text-muted);
    font-size: var(--size-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .deleting-pill {
    width: fit-content;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border-muted);
    background-color: var(--bg-elevated);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
  }

  .keyboard-shortcut-overlay {
    position: absolute;
    top: 8px;
    right: 8px;
    display: flex;
    align-items: center;
    gap: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-emphasis);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-primary);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
    z-index: 10;
    pointer-events: none;
  }

  .keyboard-shortcut-overlay .command-icon {
    color: var(--ui-accent);
    font-size: var(--size-sm);
  }

  .keyboard-shortcut-overlay .number {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    color: var(--ui-accent);
  }

  .status-indicator {
    position: absolute;
    top: 10px;
    right: 10px;
    z-index: 5;
  }

  .status-indicator.spinner {
    color: var(--ui-accent);
  }

  .status-indicator.unread-dot {
    width: 8px;
    height: 8px;
    background-color: var(--ui-accent);
    border-radius: 50%;
  }

  /* PR status colors for project icon */
  .card-header :global(.pr-status-success) {
    color: var(--ui-success);
  }

  .card-header :global(.pr-status-warning) {
    color: var(--ui-warning);
  }

  .card-header :global(.pr-status-error) {
    color: var(--ui-danger);
  }

  .card-header :global(.pr-status-pending) {
    color: var(--ui-accent);
  }

  .card-header :global(.pr-status-neutral) {
    color: var(--text-muted);
  }
</style>
