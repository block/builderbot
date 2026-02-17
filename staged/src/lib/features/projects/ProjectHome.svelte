<!--
  ProjectHome.svelte - Project workspace page

  In app navigation this is the "project page". It can render a single selected
  project (detail view) or multiple projects when no filter is provided.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowLeft } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import type { Project, Branch, StoreIncompatibility } from '../../types';
  import * as commands from '../../commands';
  import { runPrerunActions } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome } from '../../navigation.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import NewBranchModal from '../branches/NewBranchModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';

  interface Props {
    selectedProjectId?: string | null;
  }

  let { selectedProjectId = null }: Props = $props();

  // Data
  let projects = $state<Project[]>([]);
  let branchesByProject = $state<Map<string, Branch[]>>(new Map());
  let loading = $state(true);
  let error = $state<string | null>(null);

  // Store health — if non-null the DB needs a reset before we can proceed
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);

  // Modal state
  let showNewProjectModal = $state(false);
  let showNewBranchModal = $state(false);
  let newBranchProject = $state<Project | null>(null);

  // Delete confirmation state
  let projectToDelete = $state<Project | null>(null);
  let branchToDelete = $state<{ branch: Branch; project: Project } | null>(null);
  let deletingBranches = $state<Set<string>>(new Set());

  // Worktree setup errors — maps branch ID → error message
  let worktreeErrors = $state<Map<string, string>>(new Map());

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    checkStoreAndLoad();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);
    return () => window.removeEventListener('staged:new-project', onNewProject);
  });

  async function checkStoreAndLoad() {
    loading = true;
    try {
      const status = await commands.getStoreStatus();
      if (status) {
        storeIncompat = status;
        loading = false;
        return;
      }
      await loadData();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }

  async function handleResetStore() {
    resetting = true;
    try {
      await commands.confirmResetStore();
      storeIncompat = null;
      await loadData();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      resetting = false;
    }
  }

  function handleClose() {
    getCurrentWindow().close();
  }

  async function loadData() {
    loading = true;
    error = null;
    try {
      const projectList = await commands.listProjects();
      projects = projectList;

      // Load branches for each project
      const branchMap = new Map<string, Branch[]>();
      await Promise.all(
        projectList.map(async (project) => {
          const branches = await commands.listBranchesForProject(project.id);
          branchMap.set(project.id, branches);
        })
      );
      branchesByProject = branchMap;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  let visibleProjects = $derived(
    selectedProjectId ? projects.filter((project) => project.id === selectedProjectId) : projects
  );
  let hasContent = $derived(visibleProjects.length > 0);
  let selectedProject = $derived(
    selectedProjectId ? projects.find((project) => project.id === selectedProjectId) || null : null
  );

  $effect(() => {
    if (!loading && selectedProjectId && projects.length > 0 && !selectedProject) {
      goHome();
    }
  });

  // ── Project actions ──

  function handleNewProject() {
    showNewProjectModal = true;
  }

  async function handleProjectCreated(project: Project) {
    if (!projects.some((p) => p.id === project.id)) {
      projects = [...projects, project];
    }
    const branches = await commands.listBranchesForProject(project.id);
    branchesByProject = new Map(branchesByProject).set(project.id, branches);
    showNewProjectModal = false;
  }

  function handleProjectDetecting(projectId: string, detecting: boolean) {
    if (detecting) {
      detectingProjectIds = new Set([...detectingProjectIds, projectId]);
    } else {
      const next = new Set(detectingProjectIds);
      next.delete(projectId);
      detectingProjectIds = next;
    }
  }

  function handleDeleteProjectRequest(project: Project) {
    projectToDelete = project;
  }

  async function confirmDeleteProject() {
    if (!projectToDelete) return;
    const id = projectToDelete.id;
    projectToDelete = null;

    try {
      await commands.deleteProject(id);
      projects = projects.filter((p) => p.id !== id);
      const newMap = new Map(branchesByProject);
      newMap.delete(id);
      branchesByProject = newMap;
      if (selectedProjectId === id) {
        goHome();
      }
    } catch (e) {
      console.error('Failed to delete project:', e);
    }
  }

  // ── Branch actions ──

  function handleNewBranch(project: Project) {
    newBranchProject = project;
    showNewBranchModal = true;
  }

  function handleBranchCreated(branch: Branch) {
    const existing = branchesByProject.get(branch.projectId) || [];
    branchesByProject = new Map(branchesByProject).set(branch.projectId, [...existing, branch]);
    showNewBranchModal = false;
    newBranchProject = null;

    // For local branches without a worktree, set up the git worktree in the
    // background. The card will show "Creating worktree…" while this runs.
    if (branch.branchType === 'local' && !branch.worktreePath) {
      setupBranchWorktree(branch.id, branch.projectId);
    }
  }

  /** Set up a git worktree for a branch, updating the UI on success or error. */
  function setupBranchWorktree(branchId: string, projectId: string) {
    // Clear any previous error for this branch
    const nextErrors = new Map(worktreeErrors);
    nextErrors.delete(branchId);
    worktreeErrors = nextErrors;

    commands
      .setupWorktree(branchId)
      .then((updated) => {
        // Replace the branch record so the card picks up worktreePath
        const branches = branchesByProject.get(projectId) || [];
        branchesByProject = new Map(branchesByProject).set(
          projectId,
          branches.map((b) => (b.id === updated.id ? updated : b))
        );

        // Now that the worktree exists, run prerun actions
        setTimeout(() => {
          runPrerunActions(branchId, projectId).catch((e) => {
            console.error('[ProjectHome] Failed to run prerun actions:', e);
          });
        }, 150);
      })
      .catch((e) => {
        console.error('[ProjectHome] Failed to setup worktree:', e);
        const errMsg = e instanceof Error ? e.message : typeof e === 'string' ? e : String(e);
        worktreeErrors = new Map(worktreeErrors).set(branchId, errMsg);
      });
  }

  function handleDeleteBranchRequest(branchId: string, project: Project) {
    const branches = branchesByProject.get(project.id) || [];
    const branch = branches.find((b) => b.id === branchId);
    if (branch) {
      branchToDelete = { branch, project };
    }
  }

  async function confirmDeleteBranch() {
    if (!branchToDelete) return;
    const { branch } = branchToDelete;
    branchToDelete = null;

    // Show "Deleting…" state on the card immediately
    deletingBranches = new Set([...deletingBranches, branch.id]);

    try {
      await commands.deleteBranch(branch.id);
      // Remove the branch from the list on success
      const existing = branchesByProject.get(branch.projectId) || [];
      branchesByProject = new Map(branchesByProject).set(
        branch.projectId,
        existing.filter((b) => b.id !== branch.id)
      );
    } catch (e) {
      console.error('Failed to delete branch:', e);
    } finally {
      const next = new Set(deletingBranches);
      next.delete(branch.id);
      deletingBranches = next;
    }
  }

  // ── Keyboard shortcuts ──

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    if (isInput && e.key !== 'Escape') return;

    if (e.metaKey && e.key === 'n') {
      e.preventDefault();
      // If we're on a selected project page, create a new branch there.
      // Otherwise preserve current behavior.
      if (selectedProject) {
        handleNewBranch(selectedProject);
      } else if (projects.length === 1) {
        handleNewBranch(projects[0]);
      } else {
        handleNewProject();
      }
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="project-home">
  <div class="content">
    {#if loading}
      <div class="loading-state">
        <p>Loading...</p>
      </div>
    {:else if storeIncompat && storeIncompat.kind === 'needs_reset'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Required</h1>
            <span class="version-badge new">v{storeIncompat.appVersion}</span>
          </div>
          <p>
            Staged beta updates can require backwards-incompatible changes. The info stored by
            Staged (session history, notes) will be cleared, but your
            <strong>git repos and branches are not affected</strong>.
          </p>
          <div class="update-footer">
            <p class="version-hint">
              Not ready? Install <code>v{storeIncompat.dbAppVersion}</code> instead.
            </p>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
              <button class="reset-button" onclick={handleResetStore} disabled={resetting}>
                {resetting ? 'Resetting…' : 'Reset & Update'}
              </button>
            </div>
          </div>
        </div>
      </div>
    {:else if storeIncompat && storeIncompat.kind === 'too_new'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Staged</h1>
            <span class="version-badge new">v{storeIncompat.dbAppVersion}</span>
          </div>
          <p>
            This database was last used by a newer version of Staged. Please install
            <strong>v{storeIncompat.dbAppVersion}</strong> or newer to continue.
          </p>
          <div class="update-footer">
            <div></div>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
            </div>
          </div>
        </div>
      </div>
    {:else if error}
      <div class="error-state">
        <p>{error}</p>
      </div>
    {:else if !hasContent}
      <div class="empty-state">
        <div class="welcome-header">
          <StagedIcon size={28} />
          <h2>welcome to <span class="mono accent">staged</span></h2>
        </div>
        <p class="welcome-subtitle">
          Add one of your repos as a project to get started —
          <button class="kbd-btn" onclick={handleNewProject} title="New project">+</button>
          <span class="shortcut-hint">(⌘N)</span>
        </p>
        <GitTreeAnimation />
      </div>
    {:else}
      {#if selectedProject}
        <div class="project-toolbar">
          <button class="back-button" onclick={goHome} title="Back to projects list">
            <ArrowLeft size={14} />
            Projects
          </button>
          <div class="project-title">{projectDisplayName(selectedProject)}</div>
        </div>
      {/if}
      <div class="projects-list">
        {#each visibleProjects as project (project.id)}
          <ProjectSection
            {project}
            branches={branchesByProject.get(project.id) || []}
            {deletingBranches}
            {worktreeErrors}
            detecting={detectingProjectIds.has(project.id)}
            onDeleteProject={() => handleDeleteProjectRequest(project)}
            onDeleteBranch={(branchId) => handleDeleteBranchRequest(branchId, project)}
            onNewBranch={() => handleNewBranch(project)}
            onRetryWorktree={(branchId) => setupBranchWorktree(branchId, project.id)}
          />
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- New project modal -->
{#if showNewProjectModal}
  <NewProjectModal
    onCreated={handleProjectCreated}
    onDetecting={handleProjectDetecting}
    onClose={() => (showNewProjectModal = false)}
  />
{/if}

<!-- New branch modal -->
{#if showNewBranchModal && newBranchProject}
  <NewBranchModal
    project={newBranchProject}
    onCreated={handleBranchCreated}
    onClose={() => {
      showNewBranchModal = false;
      newBranchProject = null;
    }}
  />
{/if}

<!-- Delete project confirmation -->
{#if projectToDelete}
  <ConfirmDialog
    title="Remove Project"
    message={`Remove "${projectDisplayName(projectToDelete)}" from Staged? This won't delete the repository.`}
    confirmLabel="Remove"
    danger={true}
    onConfirm={confirmDeleteProject}
    onCancel={() => (projectToDelete = null)}
  />
{/if}

<!-- Delete branch confirmation -->
{#if branchToDelete}
  <ConfirmDialog
    title={branchToDelete.branch.branchType === 'remote' ? 'Delete Remote Branch' : 'Delete Branch'}
    message={branchToDelete.branch.branchType === 'remote'
      ? `Delete branch "${branchToDelete.branch.branchName}" and stop its workspace? The workspace may be reused later, but session history will be lost.`
      : `Delete branch "${branchToDelete.branch.branchName}" and its worktree? This cannot be undone.`}
    confirmLabel="Delete"
    danger={true}
    onConfirm={confirmDeleteBranch}
    onCancel={() => (branchToDelete = null)}
  />
{/if}

<style>
  .project-home {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background-color: var(--bg-chrome);
  }

  .content {
    flex: 1;
    overflow: auto;
    padding: 12px 24px 24px;
    display: flex;
    flex-direction: column;
  }

  .loading-state,
  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
    color: var(--text-muted);
  }

  .error-state {
    color: var(--ui-danger);
  }

  /* Store update state */
  .update-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
  }

  .update-card {
    width: 460px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .update-header {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .update-title {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: -0.03em;
  }

  .version-badge.new {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 4px;
    background-color: rgba(63, 185, 80, 0.12);
    color: var(--ui-accent);
  }

  .update-card > p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.6;
  }

  .update-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .version-hint {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .version-hint code {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    padding: 1px 5px;
    background-color: var(--bg-elevated);
    border-radius: 3px;
    color: var(--text-muted);
  }

  .update-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }

  .close-button {
    padding: 7px 16px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .close-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .reset-button {
    padding: 7px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 8px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 600;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .reset-button:hover {
    background-color: var(--ui-accent-hover);
  }

  .reset-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Empty state */
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

  /* Projects list */
  .projects-list {
    width: 100%;
    max-width: 800px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 32px;
  }

  .project-toolbar {
    width: 100%;
    max-width: 800px;
    margin: 0 auto 16px;
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .back-button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    padding: 6px 10px;
    font-size: var(--size-xs);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .back-button:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .project-title {
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
  }
</style>
