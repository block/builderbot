<!--
  ProjectHome.svelte — Single-project detail view

  Shows branches for a specific project. Rendered when a project is selected
  from the landing page. Includes branch management (create, delete, worktree)
  and a back button to return to the projects list.

  Data is owned by projectStore.svelte.ts — this component reads and mutates
  through the store so the sidebar can share the same reactive state.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowLeft } from 'lucide-svelte';
  import type { Project, Branch } from '../../types';
  import * as commands from '../../commands';
  import { runPrerunActions } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import {
    projectStore,
    addBranch,
    updateBranch,
    removeBranch,
    removeProject,
    setDeletingBranch,
    clearDeletingBranch,
  } from './projectStore.svelte';
  import { goHome } from '../../navigation.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import NewBranchModal from '../branches/NewBranchModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';

  interface Props {
    projectId: string;
  }

  let { projectId }: Props = $props();

  // Resolve the project from the store
  let project = $derived(projectStore.projects.find((p) => p.id === projectId));
  let branches = $derived(
    [...(projectStore.branchesByProject.get(projectId) || [])].sort(
      (a, b) => b.updatedAt - a.updatedAt
    )
  );

  // If the project was deleted (e.g. from sidebar), go back to the list
  $effect(() => {
    if (!projectStore.loading && !project) {
      goHome();
    }
  });

  // Modal state
  let showNewBranchModal = $state(false);
  let branchToDelete = $state<{ branch: Branch; project: Project } | null>(null);
  let projectToDelete = $state<Project | null>(null);

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    const onScrollToBranch = (e: Event) => {
      const branchId = (e as CustomEvent<{ branchId: string }>).detail.branchId;
      const el = document.querySelector(`[data-branch-id="${branchId}"]`);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        el.classList.add('scroll-highlight');
        setTimeout(() => el.classList.remove('scroll-highlight'), 1200);
      }
    };
    window.addEventListener('staged:scroll-to-branch', onScrollToBranch);

    // Listen for TopBar "+" button when in project detail view
    const onNewBranch = () => handleNewBranch();
    window.addEventListener('staged:new-branch', onNewBranch);

    return () => {
      window.removeEventListener('staged:scroll-to-branch', onScrollToBranch);
      window.removeEventListener('staged:new-branch', onNewBranch);
    };
  });

  // ── Branch actions ──

  function handleNewBranch() {
    showNewBranchModal = true;
  }

  function handleBranchCreated(branch: Branch) {
    addBranch(branch);
    showNewBranchModal = false;

    // For local branches without a worktree, set up the git worktree in the
    // background. The card will show "Creating worktree…" while this runs.
    if (branch.branchType === 'local' && !branch.worktreePath) {
      const branchId = branch.id;
      const bProjectId = branch.projectId;

      commands
        .setupWorktree(branchId)
        .then((updated) => {
          updateBranch(updated);
          setTimeout(() => {
            runPrerunActions(branchId, bProjectId).catch((e) => {
              console.error('[ProjectHome] Failed to run prerun actions:', e);
            });
          }, 150);
        })
        .catch((e) => {
          console.error('[ProjectHome] Failed to setup worktree:', e);
        });
    }
  }

  function handleDeleteBranchRequest(branchId: string) {
    if (!project) return;
    const branchList = projectStore.branchesByProject.get(project.id) || [];
    const branch = branchList.find((b) => b.id === branchId);
    if (branch) {
      branchToDelete = { branch, project };
    }
  }

  async function confirmDeleteBranch() {
    if (!branchToDelete) return;
    const { branch } = branchToDelete;
    branchToDelete = null;

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

  function handleDeleteProjectRequest() {
    if (project) projectToDelete = project;
  }

  async function confirmDeleteProject() {
    if (!projectToDelete) return;
    const id = projectToDelete.id;
    projectToDelete = null;

    try {
      await removeProject(id);
      // goHome() will be triggered by the $effect above when project disappears
    } catch (e) {
      console.error('Failed to delete project:', e);
    }
  }

  // ── Keyboard shortcuts ──

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    if (isInput && e.key !== 'Escape') return;

    if (e.metaKey && e.key === 'n') {
      e.preventDefault();
      handleNewBranch();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="project-home">
  <div class="content">
    {#if project}
      <div class="back-bar">
        <button class="back-button" onclick={goHome}>
          <ArrowLeft size={14} />
          <span>Projects</span>
        </button>
      </div>

      <div class="projects-list">
        <ProjectSection
          {project}
          {branches}
          deletingBranches={projectStore.deletingBranches}
          detecting={detectingProjectIds.has(project.id)}
          onDeleteProject={handleDeleteProjectRequest}
          onDeleteBranch={(branchId) => handleDeleteBranchRequest(branchId)}
          onNewBranch={handleNewBranch}
        />
      </div>
    {:else}
      <div class="loading-state">
        <p>Loading...</p>
      </div>
    {/if}
  </div>
</div>

<!-- New branch modal -->
{#if showNewBranchModal && project}
  <NewBranchModal
    {project}
    onCreated={handleBranchCreated}
    onClose={() => {
      showNewBranchModal = false;
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

  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
    color: var(--text-muted);
  }

  .back-bar {
    margin-bottom: 12px;
    max-width: 800px;
    width: 100%;
    margin-left: auto;
    margin-right: auto;
  }

  .back-button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 8px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .back-button:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
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
