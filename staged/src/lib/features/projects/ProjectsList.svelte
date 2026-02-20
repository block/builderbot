<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { GitPullRequest, GitPullRequestClosed, GitPullRequestDraft, Plus } from 'lucide-svelte';
  import type { Project, ProjectRepo, Branch } from '../../types';
  import * as commands from '../../commands';
  import {
    projectDisplayName,
    aggregateProjectPrStatus,
    projectSubtitle,
  } from '../../shared/utils';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { selectProject } from '../../navigation.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import ProjectsSidebar from './ProjectsSidebar.svelte';
  import { getProjectStatus } from './projectStatus';
  import SplashScreen from './SplashScreen.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { setHasProjects } from './projectsSidebarState.svelte';

  let projects = $state<Project[]>([]);
  let projectBranches = $state<Map<string, Branch[]>>(new Map());
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showNewProjectModal = $state(false);
  let isCommandKeyHeld = $state(false);
  let deletingProjectNames = $state<Map<string, string>>(new Map());
  let reposByProject = $state<Map<string, ProjectRepo[]>>(new Map());
  let repoLoadGeneration = 0;

  let repoCountsByProject = $derived(
    new Map(
      projects.map((p) => {
        const repos = reposByProject.get(p.id);
        return [p.id, repos ? repos.length : p.githubRepo ? 1 : 0] as const;
      })
    )
  );

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

    // Listen for PR status changes to update branch state
    let unlistenPrStatus: UnlistenFn | undefined;
    listen<{
      branchId: string;
      prState: string;
      prChecksStatus: string;
      prReviewDecision: string | null;
      prMergeable: boolean;
      prDraft: boolean;
    }>('pr-status-changed', (event) => {
      const payload = event.payload;
      // Find the project that contains this branch
      for (const [projectId, branches] of projectBranches.entries()) {
        const branchIndex = branches.findIndex((b) => b.id === payload.branchId);
        if (branchIndex !== -1) {
          // Update the branch with new PR status
          const updatedBranches = [...branches];
          updatedBranches[branchIndex] = {
            ...updatedBranches[branchIndex],
            prState: payload.prState,
            prChecksStatus: payload.prChecksStatus,
            prReviewDecision: payload.prReviewDecision,
            prMergeable: payload.prMergeable,
            prDraft: payload.prDraft,
          };
          projectBranches = new Map(projectBranches).set(projectId, updatedBranches);
          break;
        }
      }
    }).then((unlisten) => {
      unlistenPrStatus = unlisten;
    });

    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
      window.removeEventListener('staged:project-delete-start', onProjectDeleteStart);
      window.removeEventListener('staged:project-delete-end', onProjectDeleteEnd);
      unlistenPrStatus?.();
    };
  });

  async function loadProjects() {
    loading = true;
    error = null;
    try {
      const loadedProjects = await commands.listProjects();
      projects = loadedProjects;
      setHasProjects(loadedProjects.length > 0);
      void hydrateRepos(loadedProjects);
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

  async function hydrateRepos(projectList: Project[]) {
    const generation = ++repoLoadGeneration;
    const entries = await Promise.all(
      projectList.map(async (project) => {
        try {
          const repos = await commands.listProjectRepos(project.id);
          return [project.id, repos] as const;
        } catch (e) {
          console.error(`[ProjectsList] Failed to load repos for project '${project.id}':`, e);
          return [project.id, [] as ProjectRepo[]] as const;
        }
      })
    );
    if (generation !== repoLoadGeneration) return;
    reposByProject = new Map(entries);
  }

  function handleProjectCreated(project: Project) {
    if (!projects.some((p) => p.id === project.id)) {
      projects = [...projects, project];
    }
    void hydrateRepos(projects);
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
  ): 'merged' | 'open' | 'closed' | 'checks_failing' | 'conflict' | null {
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
    {projectBranches}
    showAllProjectsRow={true}
  />

  <div class="main-panel">
    <div class="content" class:empty-layout={!loading && !error && projects.length === 0}>
      {#if loading}
        <div class="state">Loading projects…</div>
      {:else if error}
        <div class="state error">{error}</div>
      {:else if projects.length === 0}
        <SplashScreen
          onCreated={handleProjectCreated}
          requestOpen={showNewProjectModal && projects.length === 0}
          onFormOpenChange={(open) => (showNewProjectModal = open)}
        />
      {:else}
        <div class="title-row">
          <h1>Projects</h1>
          <button class="new-project-btn" onclick={() => (showNewProjectModal = true)}>
            <Plus size={14} />
            New project
          </button>
        </div>
        <div class="projects-grid">
          {#each projects as project, index (project.id)}
            {@const status = getProjectStatus(project.id, deletingProjectNames)}
            {@const prStatus = getProjectPrStatus(project.id)}
            {@const repos = reposByProject.get(project.id) ?? []}
            {@const repoCount = repoCountsByProject.get(project.id) ?? (project.githubRepo ? 1 : 0)}
            {@const sessionTypes = projectStateStore.getRunningSessionTypes(project.id)}
            <div class="project-card-wrapper">
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
                  <div
                    class="status-indicator spinner"
                    in:fade={{ duration: 300, delay: 150 }}
                    out:fade={{ duration: 150 }}
                  >
                    <Spinner size={14} />
                  </div>
                {:else if status.kind === 'unread'}
                  <div
                    class="status-indicator unread-dot"
                    in:fade={{ duration: 300, delay: 150 }}
                    out:fade={{ duration: 150 }}
                  ></div>
                {/if}
                <div class="card-header">
                  {#if prStatus === 'merged'}
                    <GitPullRequest size={16} class="pr-status-merged" />
                  {:else if prStatus === 'checks_failing'}
                    <GitPullRequest size={16} class="pr-status-checks-failing" />
                  {:else if prStatus === 'open'}
                    <GitPullRequest size={16} />
                  {:else if prStatus === 'closed'}
                    <GitPullRequestClosed size={16} />
                  {:else if prStatus === 'conflict'}
                    <GitPullRequestClosed size={16} class="pr-status-conflict" />
                  {:else}
                    <GitPullRequestDraft size={16} class="pr-status-draft" />
                  {/if}
                  <span>{projectDisplayName(project)}</span>
                </div>
                {#if status.kind === 'deleting'}
                  <div class="deleting-pill" role="status" aria-live="polite">Deleting…</div>
                {/if}
                <div class="repo">
                  {#if repos.length > 0}
                    {#each repos as r, i}
                      {#if i > 0}<span class="repo-separator">, </span>{/if}
                      <RepoLabel githubRepo={r.githubRepo} subpath={r.subpath} />
                    {/each}
                  {:else if project.githubRepo}
                    <RepoLabel githubRepo={project.githubRepo} subpath={project.subpath} />
                  {:else}
                    No repo attached
                  {/if}
                </div>
              </button>
              <div class="card-location">
                {projectSubtitle(repoCount, sessionTypes)}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

{#if showNewProjectModal && projects.length > 0}
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
    padding: 0;
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  .title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .title-row h1 {
    margin: 0;
    font-size: var(--size-xl);
    font-weight: 700;
    color: var(--text-primary);
  }

  .new-project-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border: none;
    border-radius: 8px;
    background-color: var(--bg-elevated);
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .new-project-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .projects-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    grid-auto-rows: 1fr;
    gap: 12px;
  }

  .project-card-wrapper {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .project-card-wrapper .project-card {
    flex: 1;
  }

  .card-location {
    color: var(--text-faint);
    font-size: var(--size-xs);
    padding: 0 4px;
  }

  .project-card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
    text-align: left;
    background: var(--bg-elevated);
    border: none;
    border-radius: 10px;
    padding: 16px;
    min-height: 120px;
    color: inherit;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .project-card:hover {
    background-color: var(--bg-hover);
  }

  .project-card:disabled {
    cursor: not-allowed;
  }

  .project-card.deleting {
    opacity: 0.75;
  }

  .project-card.deleting:hover {
    background: var(--bg-elevated);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-primary);
    font-size: var(--size-lg);
    font-weight: 600;
    padding-right: 24px;
  }

  .card-header :global(svg) {
    flex-shrink: 0;
  }

  .card-header :global(svg.pr-status-merged) {
    stroke: var(--ui-success);
  }

  .card-header :global(svg.pr-status-conflict) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.pr-status-checks-failing) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.pr-status-draft) {
    stroke: var(--text-muted);
  }

  .repo {
    margin-top: auto;
    font-size: var(--size-sm);
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
  }

  .repo-separator {
    color: var(--text-faint);
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
