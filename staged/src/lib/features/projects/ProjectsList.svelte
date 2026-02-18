<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { FolderGit2 } from 'lucide-svelte';
  import type { Project } from '../../types';
  import * as commands from '../../commands';
  import { projectDisplayName } from '../../shared/utils';
  import { selectProject } from '../../navigation.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';

  let projects = $state<Project[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showNewProjectModal = $state(false);
  let isCommandKeyHeld = $state(false);
  let deletingProjectNames = $state<Map<string, string>>(new Map());

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
    selectProject(project.id);
  }

  function isProjectDeleting(projectId: string): boolean {
    return deletingProjectNames.has(projectId);
  }

  function openProject(projectId: string) {
    if (isProjectDeleting(projectId)) return;
    selectProject(projectId);
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
  <div class="content" class:empty-layout={!loading && !error && projects.length === 0}>
    {#if !loading && !error && projects.length > 0}
      <div class="header">
        <h1>Projects</h1>
        <span class="count">{projects.length}</span>
      </div>
    {/if}

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
          <button class="kbd-btn" onclick={() => (showNewProjectModal = true)} title="New project">
            +
          </button>
          <span class="shortcut-hint">(⌘N)</span>
        </p>
        <GitTreeAnimation />
      </div>
    {:else}
      <div class="projects-grid">
        <button class="project-card new-project-card" onclick={() => (showNewProjectModal = true)}>
          <div class="new-project-content">
            <span class="new-project-label">+ New project</span>
          </div>
        </button>
        {#each projects as project, index (project.id)}
          {@const hasRunning = projectStateStore.hasRunningSessions(project.id)}
          {@const isUnread = projectStateStore.isUnread(project.id)}
          <button
            class="project-card"
            class:deleting={isProjectDeleting(project.id)}
            onclick={() => openProject(project.id)}
            disabled={isProjectDeleting(project.id)}
            title={isProjectDeleting(project.id) ? 'Project deletion in progress' : undefined}
          >
            {#if isCommandKeyHeld && index < 9}
              <div class="keyboard-shortcut-overlay">
                <span class="command-icon">⌘</span>
                <span class="number">{index + 1}</span>
              </div>
            {/if}
            {#if hasRunning}
              <div class="status-indicator spinner">
                <Spinner size={14} />
              </div>
            {:else if isUnread}
              <div class="status-indicator unread-dot"></div>
            {/if}
            <div class="card-header">
              <FolderGit2 size={16} width={16} height={16} />
              <span>{projectDisplayName(project)}</span>
            </div>
            {#if isProjectDeleting(project.id)}
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

{#if showNewProjectModal}
  <NewProjectModal onCreated={handleProjectCreated} onClose={() => (showNewProjectModal = false)} />
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
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .content.empty-layout {
    max-width: none;
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

  .card-header :global(svg) {
    flex-shrink: 0;
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
</style>
