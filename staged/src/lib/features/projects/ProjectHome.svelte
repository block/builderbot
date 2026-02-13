<!--
  ProjectHome.svelte - The single main page

  Shows all projects with their branches. Empty state when no projects exist.
  Modals for creating/deleting projects and branches are layered on top.

  Data is owned by projectStore.svelte.ts — this component reads and mutates
  through the store so the sidebar can share the same reactive state.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import type { Project, Branch } from '../../types';
  import * as commands from '../../commands';
  import { runPrerunActions } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import {
    projectStore,
    checkStoreAndLoad,
    handleResetStore,
    addProject,
    removeProject,
    addBranch,
    updateBranch,
    removeBranch,
    setDeletingBranch,
    clearDeletingBranch,
  } from './projectStore.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import NewBranchModal from '../branches/NewBranchModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';

  // Modal state (local to this component)
  let showNewProjectModal = $state(false);
  let showNewBranchModal = $state(false);
  let newBranchProject = $state<Project | null>(null);

  // Delete confirmation state
  let projectToDelete = $state<Project | null>(null);
  let branchToDelete = $state<{ branch: Branch; project: Project } | null>(null);

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    checkStoreAndLoad();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);

    const onScrollToBranch = (e: Event) => {
      const branchId = (e as CustomEvent<{ branchId: string }>).detail.branchId;
      const el = document.querySelector(`[data-branch-id="${branchId}"]`);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        // Brief highlight flash to draw attention
        el.classList.add('scroll-highlight');
        setTimeout(() => el.classList.remove('scroll-highlight'), 1200);
      }
    };
    window.addEventListener('staged:scroll-to-branch', onScrollToBranch);

    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
      window.removeEventListener('staged:scroll-to-branch', onScrollToBranch);
    };
  });

  function handleClose() {
    getCurrentWindow().close();
  }

  let hasContent = $derived(projectStore.projects.length > 0);

  // ── Project actions ──

  function handleNewProject() {
    showNewProjectModal = true;
  }

  async function handleProjectCreated(project: Project) {
    await addProject(project);
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
      await removeProject(id);
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
    addBranch(branch);
    showNewBranchModal = false;
    newBranchProject = null;

    // For local branches without a worktree, set up the git worktree in the
    // background. The card will show "Creating worktree…" while this runs.
    if (branch.branchType === 'local' && !branch.worktreePath) {
      const branchId = branch.id;
      const projectId = branch.projectId;

      commands
        .setupWorktree(branchId)
        .then((updated) => {
          // Replace the branch record so the card picks up worktreePath
          updateBranch(updated);

          // Now that the worktree exists, run prerun actions
          setTimeout(() => {
            runPrerunActions(branchId, projectId).catch((e) => {
              console.error('[ProjectHome] Failed to run prerun actions:', e);
            });
          }, 150);
        })
        .catch((e) => {
          console.error('[ProjectHome] Failed to setup worktree:', e);
        });
    }
  }

  function handleDeleteBranchRequest(branchId: string, project: Project) {
    const branches = projectStore.branchesByProject.get(project.id) || [];
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
    setDeletingBranch(branch.id);

    try {
      await commands.deleteBranch(branch.id);
      removeBranch(branch);
    } catch (e) {
      console.error('Failed to delete branch:', e);
    } finally {
      clearDeletingBranch(branch.id);
    }
  }

  // ── Keyboard shortcuts ──

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    if (isInput && e.key !== 'Escape') return;

    if (e.metaKey && e.key === 'n') {
      e.preventDefault();
      // If there's exactly one project, open new branch for it.
      // Otherwise, open new project.
      if (projectStore.projects.length === 1) {
        handleNewBranch(projectStore.projects[0]);
      } else {
        handleNewProject();
      }
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="project-home">
  <div class="content">
    {#if projectStore.loading}
      <div class="loading-state">
        <p>Loading...</p>
      </div>
    {:else if projectStore.storeIncompat && projectStore.storeIncompat.kind === 'needs_reset'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Required</h1>
            <span class="version-badge new">v{projectStore.storeIncompat.appVersion}</span>
          </div>
          <p>
            Staged beta updates can require backwards-incompatible changes. The info stored by
            Staged (session history, notes) will be cleared, but your
            <strong>git repos and branches are not affected</strong>.
          </p>
          <div class="update-footer">
            <p class="version-hint">
              Not ready? Install <code>v{projectStore.storeIncompat.dbAppVersion}</code> instead.
            </p>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
              <button
                class="reset-button"
                onclick={handleResetStore}
                disabled={projectStore.resetting}
              >
                {projectStore.resetting ? 'Resetting…' : 'Reset & Update'}
              </button>
            </div>
          </div>
        </div>
      </div>
    {:else if projectStore.storeIncompat && projectStore.storeIncompat.kind === 'too_new'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Staged</h1>
            <span class="version-badge new">v{projectStore.storeIncompat.dbAppVersion}</span>
          </div>
          <p>
            This database was last used by a newer version of Staged. Please install
            <strong>v{projectStore.storeIncompat.dbAppVersion}</strong> or newer to continue.
          </p>
          <div class="update-footer">
            <div></div>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
            </div>
          </div>
        </div>
      </div>
    {:else if projectStore.error}
      <div class="error-state">
        <p>{projectStore.error}</p>
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
      <div class="projects-list">
        {#each projectStore.projects as project (project.id)}
          <ProjectSection
            {project}
            branches={[...(projectStore.branchesByProject.get(project.id) || [])].sort(
              (a, b) => b.updatedAt - a.updatedAt
            )}
            deletingBranches={projectStore.deletingBranches}
            detecting={detectingProjectIds.has(project.id)}
            onDeleteProject={() => handleDeleteProjectRequest(project)}
            onDeleteBranch={(branchId) => handleDeleteBranchRequest(branchId, project)}
            onNewBranch={() => handleNewBranch(project)}
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

  /* Scroll-to highlight animation (applied via JS on branch cards) */
  :global(.scroll-highlight) {
    animation: scroll-highlight-flash 1.2s ease-out;
  }

  @keyframes scroll-highlight-flash {
    0% {
      box-shadow: 0 0 0 2px var(--ui-accent);
    }
    100% {
      box-shadow: 0 0 0 2px transparent;
    }
  }
</style>
