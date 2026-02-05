<!--
  BranchHome.svelte - Branch-based workflow homepage

  Shows all tracked branches grouped by project, with their commit stacks.
  Each branch has a worktree for isolated development.

  Keyboard shortcuts:
  - Cmd+N: New branch
  - Escape: Close modals
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Plus, Sparkles, Folder, GitBranch, Loader2, X } from 'lucide-svelte';
  import type { Branch, GitProject } from './services/branch';
  import * as branchService from './services/branch';
  import { listenToSessionStatus, type SessionStatusEvent } from './services/ai';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import BranchCard from './BranchCard.svelte';
  import NewBranchModal, { type PendingBranch } from './NewBranchModal.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { DiffSpec } from './types';

  interface Props {
    onViewDiff?: (projectId: string, repoPath: string, spec: DiffSpec, label: string) => void;
    onAddProjectRequest?: (trigger: () => void) => void;
  }

  let { onViewDiff, onAddProjectRequest }: Props = $props();

  // State
  let branches = $state<Branch[]>([]);
  let projects = $state<GitProject[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshKey = $state(0);

  // Pending branches (being created asynchronously)
  let pendingBranches = $state<PendingBranch[]>([]);
  // Failed branch creations (to show error state)
  let failedBranches = $state<Map<string, string>>(new Map());

  // Branches currently being deleted (show spinner)
  let deletingBranchIds = $state<Set<string>>(new Set());
  // Failed branch deletions (to show error state)
  let deleteErrors = $state<Map<string, string>>(new Map());

  // Event listener cleanup
  let unlistenStatus: UnlistenFn | null = null;

  // Modal state
  let showNewBranchModal = $state(false);
  let newBranchForProject = $state<GitProject | null>(null);
  let branchToDelete = $state<Branch | null>(null);
  let showNewProjectModal = $state(false);

  // Expose the add project trigger to parent (top bar "Add Project" button)
  $effect(() => {
    onAddProjectRequest?.(() => {
      showNewProjectModal = true;
    });
  });

  // Group branches by project (including pending ones)
  // Empty projects are filtered out at render time
  let branchesByProject = $derived.by(() => {
    const grouped = new Map<
      string,
      { project: GitProject; branches: Branch[]; pending: PendingBranch[] }
    >();

    // Seed all projects
    for (const project of projects) {
      grouped.set(project.id, { project, branches: [], pending: [] });
    }

    // Add branches to their specific project
    for (const branch of branches) {
      const projectGroup = grouped.get(branch.projectId);
      if (projectGroup) {
        projectGroup.branches.push(branch);
      }
    }

    // Add pending branches to their specific project
    for (const pending of pendingBranches) {
      const projectGroup = grouped.get(pending.projectId);
      if (projectGroup) {
        projectGroup.pending.push(pending);
      }
    }

    return grouped;
  });

  // Show the main list when there's anything to display
  let hasContent = $derived(
    branches.length > 0 || pendingBranches.length > 0 || projects.length > 0
  );

  // Generate a unique key for a pending branch
  function pendingKey(pending: PendingBranch): string {
    return `${pending.projectId}:${pending.branchName}`;
  }

  function projectDisplayName(project: GitProject): string {
    const repoName = project.repoPath.split('/').pop() || project.repoPath;
    return project.subpath ? `${repoName}/${project.subpath}` : repoName;
  }

  // Load branches and projects on mount and set up session status listener
  onMount(async () => {
    await loadData();

    // Listen for AI session status changes to update branch sessions
    unlistenStatus = await listenToSessionStatus(handleSessionStatus);
  });

  /**
   * Handle AI session status changes.
   * When an AI session transitions to 'idle', look up the corresponding branch session
   * or note and mark it as completed.
   */
  async function handleSessionStatus(event: SessionStatusEvent) {
    // Only care about transitions to idle (session complete)
    if (event.status.status !== 'idle') {
      return;
    }

    // First, check if this is a branch session (commit)
    const branchSession = await branchService.getBranchSessionByAiSession(event.sessionId);
    if (branchSession && branchSession.status === 'running') {
      console.log('AI session completed, recovering branch session:', branchSession.id);
      await branchService.recoverOrphanedSession(branchSession.branchId);
      refreshKey++;
      return;
    }

    // Next, check if this is a branch note
    const branchNote = await branchService.getBranchNoteByAiSession(event.sessionId);
    if (branchNote && branchNote.status === 'generating') {
      console.log('AI session completed, recovering branch note:', branchNote.id);
      await branchService.recoverOrphanedNote(branchNote.branchId);
      refreshKey++;
      return;
    }
  }

  async function loadData() {
    loading = true;
    error = null;
    try {
      // Load branches and projects in parallel
      const [branchList, projectList] = await Promise.all([
        branchService.listBranches(),
        branchService.listGitProjects(),
      ]);
      branches = branchList;
      projects = projectList;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function handleNewBranch(project?: GitProject) {
    newBranchForProject = project || null;
    showNewBranchModal = true;
  }

  async function handleBranchCreating(pending: PendingBranch) {
    // Ensure project exists in our local list
    let project = projects.find((p) => p.id === pending.projectId);
    if (!project) {
      try {
        // Fetch the project that was created
        const fetchedProject = await branchService.getGitProject(pending.projectId);
        if (fetchedProject) {
          projects = [...projects, fetchedProject];
        }
      } catch (e) {
        console.error('Failed to fetch project:', pending.projectId, e);
      }
    }

    // Add to pending list and close modal immediately
    pendingBranches = [...pendingBranches, pending];
    // Clear any previous failure for this branch
    const key = pendingKey(pending);
    if (failedBranches.has(key)) {
      const newFailed = new Map(failedBranches);
      newFailed.delete(key);
      failedBranches = newFailed;
    }
    showNewBranchModal = false;
    newBranchForProject = null;
  }

  function handleBranchCreated(branch: Branch) {
    // Remove from pending and add to real branches
    pendingBranches = pendingBranches.filter(
      (p) => !(p.projectId === branch.projectId && p.branchName === branch.branchName)
    );
    branches = [...branches, branch];
  }

  function handleBranchCreateFailed(pending: PendingBranch, errorMsg: string) {
    // Mark as failed (keep in pending list but show error state)
    const key = pendingKey(pending);
    failedBranches = new Map(failedBranches).set(key, errorMsg);
  }

  function dismissFailedBranch(pending: PendingBranch) {
    const key = pendingKey(pending);
    pendingBranches = pendingBranches.filter((p) => pendingKey(p) !== key);
    const newFailed = new Map(failedBranches);
    newFailed.delete(key);
    failedBranches = newFailed;
  }

  async function handleDeleteBranch(branchId: string) {
    const branch = branches.find((b) => b.id === branchId);
    if (!branch) return;

    // Show confirmation dialog
    branchToDelete = branch;
  }

  async function confirmDeleteBranch() {
    if (!branchToDelete) return;

    const id = branchToDelete.id;
    // Close dialog and show spinner immediately
    branchToDelete = null;
    deletingBranchIds = new Set(deletingBranchIds).add(id);

    try {
      await branchService.deleteBranch(id);
      // Success: remove branch and spinner
      branches = branches.filter((b) => b.id !== id);
      const newDeleting = new Set(deletingBranchIds);
      newDeleting.delete(id);
      deletingBranchIds = newDeleting;
    } catch (e) {
      // Failure: remove spinner, show error card
      const newDeleting = new Set(deletingBranchIds);
      newDeleting.delete(id);
      deletingBranchIds = newDeleting;
      deleteErrors = new Map(deleteErrors).set(id, e instanceof Error ? e.message : String(e));
    }
  }

  function dismissDeleteError(branchId: string) {
    const newErrors = new Map(deleteErrors);
    newErrors.delete(branchId);
    deleteErrors = newErrors;
  }

  function handleViewDiff(branch: Branch) {
    onViewDiff?.(
      branch.projectId,
      branch.worktreePath,
      DiffSpec.fromRevs(branch.baseBranch, branch.branchName),
      `${branch.baseBranch}..${branch.branchName}`
    );
  }

  function handleViewCommitDiff(branch: Branch, commitSha: string) {
    onViewDiff?.(
      branch.projectId,
      branch.worktreePath,
      DiffSpec.fromRevs(`${commitSha}~1`, commitSha),
      commitSha.slice(0, 7)
    );
  }

  function handleNewProjectCreated(project: GitProject) {
    projects = [...projects, project];
    showNewProjectModal = false;
  }

  // Keyboard shortcuts
  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';

    if (isInput && e.key !== 'Escape') {
      return;
    }

    // Cmd+N - New branch
    if (e.metaKey && e.key === 'n') {
      e.preventDefault();
      handleNewBranch();
      return;
    }

    // Escape - Close modals
    if (e.key === 'Escape') {
      if (showNewProjectModal) {
        e.preventDefault();
        showNewProjectModal = false;
      } else if (showNewBranchModal) {
        e.preventDefault();
        showNewBranchModal = false;
        newBranchForProject = null;
      }
      return;
    }
  }

  onMount(() => {
    window.addEventListener('keydown', handleKeydown);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleKeydown);
    unlistenStatus?.();
  });
</script>

<div class="branch-home">
  <div class="content">
    {#if loading}
      <div class="loading-state">
        <p>Loading...</p>
      </div>
    {:else if error}
      <div class="error-state">
        <p>{error}</p>
      </div>
    {:else if !hasContent}
      <div class="empty-state">
        <Sparkles size={48} strokeWidth={1} />
        <h2>Welcome to Staged</h2>
        <p>Create a branch to start working</p>
        <button class="create-button" onclick={() => handleNewBranch()}>
          <Plus size={16} />
          New Branch
        </button>
        <span class="shortcut-hint">or press ⌘N</span>
      </div>
    {:else}
      <!-- Branches grouped by project -->
      <div class="projects-list">
        {#each [...branchesByProject.entries()].filter(([_, { branches: b, pending: p }]) => b.length > 0 || p.length > 0) as [projectId, { project, branches: projectBranches, pending: projectPending }] (projectId)}
          <div class="project-section">
            <div class="project-header">
              <div class="project-info">
                <Folder size={14} class="project-icon" />
                <span class="project-name">{projectDisplayName(project)}</span>
              </div>
            </div>
            <div class="branches-list">
              {#each projectBranches as branch (branch.id)}
                {@const isDeleting = deletingBranchIds.has(branch.id)}
                {@const deleteFailed = deleteErrors.get(branch.id)}
                {#if isDeleting || deleteFailed}
                  <div class="pending-branch-card" class:failed={!!deleteFailed}>
                    <div class="pending-header">
                      <div class="pending-info">
                        <GitBranch size={16} class="pending-branch-icon" />
                        <span class="pending-branch-name">{branch.branchName}</span>
                        <span class="pending-separator">›</span>
                        <span class="pending-base-branch"
                          >{branch.baseBranch.replace(/^origin\//, '')}</span
                        >
                      </div>
                      {#if deleteFailed}
                        <button
                          class="dismiss-button"
                          onclick={() => dismissDeleteError(branch.id)}
                          title="Dismiss"
                        >
                          <X size={14} />
                        </button>
                      {/if}
                    </div>
                    <div class="pending-content">
                      {#if deleteFailed}
                        <div class="pending-error">
                          <span class="error-label">Failed to delete branch:</span>
                          <span class="error-message">{deleteFailed}</span>
                        </div>
                      {:else}
                        <div class="pending-status">
                          <Loader2 size={14} class="spinner" />
                          <span>Removing worktree...</span>
                        </div>
                      {/if}
                    </div>
                  </div>
                {:else}
                  <BranchCard
                    {branch}
                    {refreshKey}
                    onViewDiff={() => handleViewDiff(branch)}
                    onViewCommitDiff={(sha) => handleViewCommitDiff(branch, sha)}
                    onDelete={() => handleDeleteBranch(branch.id)}
                  />
                {/if}
              {/each}
              <!-- Pending branches (being created) -->
              {#each projectPending as pending (pendingKey(pending))}
                {@const failed = failedBranches.get(pendingKey(pending))}
                <div class="pending-branch-card" class:failed={!!failed}>
                  <div class="pending-header">
                    <div class="pending-info">
                      <GitBranch size={16} class="pending-branch-icon" />
                      <span class="pending-branch-name">{pending.branchName}</span>
                      <span class="pending-separator">›</span>
                      <span class="pending-base-branch"
                        >{pending.baseBranch.replace(/^origin\//, '')}</span
                      >
                    </div>
                    {#if failed}
                      <button
                        class="dismiss-button"
                        onclick={() => dismissFailedBranch(pending)}
                        title="Dismiss"
                      >
                        <X size={14} />
                      </button>
                    {/if}
                  </div>
                  <div class="pending-content">
                    {#if failed}
                      <div class="pending-error">
                        <span class="error-label">Failed to create branch:</span>
                        <span class="error-message">{failed}</span>
                      </div>
                    {:else}
                      <div class="pending-status">
                        <Loader2 size={14} class="spinner" />
                        <span>Setting up worktree...</span>
                      </div>
                    {/if}
                  </div>
                </div>
              {/each}
              <!-- Per-project new branch button -->
              <button class="new-branch-button" onclick={() => handleNewBranch(project)}>
                <Plus size={16} />
                New Branch
              </button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- New project modal -->
{#if showNewProjectModal}
  <NewProjectModal
    onCreated={handleNewProjectCreated}
    onClose={() => (showNewProjectModal = false)}
  />
{/if}

<!-- New branch modal -->
{#if showNewBranchModal}
  <NewBranchModal
    initialRepoPath={newBranchForProject?.repoPath}
    projectId={newBranchForProject?.id}
    onCreating={handleBranchCreating}
    onCreated={handleBranchCreated}
    onCreateFailed={handleBranchCreateFailed}
    onClose={() => {
      showNewBranchModal = false;
      newBranchForProject = null;
    }}
  />
{/if}

<!-- Delete confirmation dialog -->
{#if branchToDelete}
  <ConfirmDialog
    title="Delete Branch"
    message={`Delete branch "${branchToDelete.branchName}" and its worktree? This cannot be undone.`}
    confirmLabel="Delete"
    danger={true}
    onConfirm={confirmDeleteBranch}
    onCancel={() => (branchToDelete = null)}
  />
{/if}

<style>
  .branch-home {
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
  }

  .loading-state,
  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
  }

  .error-state {
    color: var(--ui-danger);
  }

  /* Empty state */
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    gap: 16px;
    color: var(--text-muted);
  }

  .empty-state h2 {
    font-size: var(--size-xl);
    font-weight: 500;
    color: var(--text-primary);
    margin: 0;
  }

  .empty-state p {
    margin: 0;
    color: var(--text-muted);
  }

  .create-button {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 8px;
    color: var(--bg-deepest);
    font-size: var(--size-md);
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .create-button:hover {
    background-color: var(--ui-accent-hover);
  }

  .shortcut-hint {
    font-size: var(--size-sm);
    color: var(--text-faint);
  }

  /* Projects list */
  .projects-list {
    max-width: 800px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 32px;
  }

  .project-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .project-header {
    display: flex;
    align-items: center;
    padding: 0 4px;
  }

  .project-info {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  :global(.project-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .project-name {
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .branches-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .new-branch-button {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    background-color: transparent;
    border: 1px dashed var(--border-muted);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--size-md);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .new-branch-button:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }

  /* Pending branch card */
  .pending-branch-card {
    display: flex;
    flex-direction: column;
    background-color: var(--bg-primary);
    border-radius: 8px;
    overflow: hidden;
  }

  .pending-branch-card.failed {
    border: 1px solid var(--ui-danger-bg);
  }

  .pending-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .pending-info {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  :global(.pending-branch-icon) {
    color: var(--status-renamed);
  }

  .pending-branch-name {
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .pending-separator {
    color: var(--text-faint);
    font-size: var(--size-md);
  }

  .pending-base-branch {
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-muted);
  }

  .dismiss-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .dismiss-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .pending-content {
    padding: 12px 16px;
  }

  .pending-status {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .pending-error {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .error-label {
    font-size: var(--size-sm);
    color: var(--ui-danger);
    font-weight: 500;
  }

  .error-message {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  /* Spinner animation */
  :global(.spinner) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
