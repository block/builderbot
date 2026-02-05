<!--
  BranchHome.svelte - Branch-based workflow homepage

  Shows all tracked branches grouped by repository, with their commit stacks.
  Each branch has a worktree for isolated development.

  Keyboard shortcuts:
  - Cmd+N: New branch
  - Escape: Close modals
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Plus, Sparkles, Folder, GitBranch, Loader2, X } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import * as branchService from './services/branch';
  import { listenToSessionStatus, type SessionStatusEvent } from './services/ai';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import BranchCard from './BranchCard.svelte';
  import NewBranchModal, { type PendingBranch } from './NewBranchModal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { DiffSpec } from './types';

  interface Props {
    onViewDiff?: (repoPath: string, spec: DiffSpec, label: string) => void;
    onNewBranchRequest?: (trigger: () => void) => void;
  }

  let { onViewDiff, onNewBranchRequest }: Props = $props();

  // State
  let branches = $state<Branch[]>([]);
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
  let branchToDelete = $state<Branch | null>(null);

  // Expose the new branch trigger to parent
  $effect(() => {
    onNewBranchRequest?.(() => {
      showNewBranchModal = true;
    });
  });

  // Group branches by repo path (including pending ones)
  let branchesByRepo = $derived.by(() => {
    const grouped = new Map<string, { branches: Branch[]; pending: PendingBranch[] }>();

    // Add real branches
    for (const branch of branches) {
      const existing = grouped.get(branch.repoPath) || { branches: [], pending: [] };
      existing.branches.push(branch);
      grouped.set(branch.repoPath, existing);
    }

    // Add pending branches
    for (const pending of pendingBranches) {
      const existing = grouped.get(pending.repoPath) || { branches: [], pending: [] };
      existing.pending.push(pending);
      grouped.set(pending.repoPath, existing);
    }

    return grouped;
  });

  // Check if we have any branches or pending branches
  let hasBranches = $derived(branches.length > 0 || pendingBranches.length > 0);

  // Extract repo name from path
  function repoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
  }

  // Generate a unique key for a pending branch
  function pendingKey(pending: PendingBranch): string {
    return `${pending.repoPath}:${pending.branchName}`;
  }

  // Load branches on mount and set up session status listener
  onMount(async () => {
    await loadBranches();

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

  async function loadBranches() {
    loading = true;
    error = null;
    try {
      branches = await branchService.listBranches();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function handleNewBranch() {
    showNewBranchModal = true;
  }

  function handleBranchCreating(pending: PendingBranch) {
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
  }

  function handleBranchCreated(branch: Branch) {
    // Remove from pending and add to real branches
    pendingBranches = pendingBranches.filter(
      (p) => !(p.repoPath === branch.repoPath && p.branchName === branch.branchName)
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
      branch.worktreePath,
      DiffSpec.fromRevs(branch.baseBranch, branch.branchName),
      `${branch.baseBranch}..${branch.branchName}`
    );
  }

  function handleViewCommitDiff(branch: Branch, commitSha: string) {
    onViewDiff?.(
      branch.worktreePath,
      DiffSpec.fromRevs(`${commitSha}~1`, commitSha),
      commitSha.slice(0, 7)
    );
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
      if (showNewBranchModal) {
        e.preventDefault();
        showNewBranchModal = false;
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
    {:else if !hasBranches}
      <div class="empty-state">
        <Sparkles size={48} strokeWidth={1} />
        <h2>Welcome to Staged</h2>
        <p>Create a branch to start working</p>
        <button class="create-button" onclick={handleNewBranch}>
          <Plus size={16} />
          New Branch
        </button>
        <span class="shortcut-hint">or press ⌘N</span>
      </div>
    {:else}
      <!-- Branches grouped by repo -->
      <div class="repos-list">
        {#each [...branchesByRepo.entries()] as [repoPath, { branches: repoBranches, pending: repoPending }] (repoPath)}
          <div class="repo-section">
            <div class="repo-header">
              <Folder size={14} class="repo-icon" />
              <span class="repo-name">{repoName(repoPath)}</span>
              <span class="repo-path">{repoPath}</span>
            </div>
            <div class="branches-list">
              {#each repoBranches as branch (branch.id)}
                {@const isDeleting = deletingBranchIds.has(branch.id)}
                {@const deleteFailed = deleteErrors.get(branch.id)}
                {#if isDeleting || deleteFailed}
                  <div class="pending-branch-card" class:failed={!!deleteFailed}>
                    <div class="pending-header">
                      <div class="pending-info">
                        <GitBranch size={16} class="pending-branch-icon" />
                        <span class="pending-branch-name">{branch.branchName}</span>
                        <span class="pending-separator">›</span>
                        <span class="pending-base-branch">{branch.baseBranch.replace(/^origin\//, '')}</span>
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
              {#each repoPending as pending (pendingKey(pending))}
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
            </div>
          </div>
        {/each}

        <!-- New branch button at bottom -->
        <div class="new-branch-section">
          <button class="new-branch-button" onclick={handleNewBranch}>
            <Plus size={16} />
            New Branch
          </button>
        </div>
      </div>
    {/if}
  </div>
</div>

<!-- New branch modal -->
{#if showNewBranchModal}
  <NewBranchModal
    onCreating={handleBranchCreating}
    onCreated={handleBranchCreated}
    onCreateFailed={handleBranchCreateFailed}
    onClose={() => (showNewBranchModal = false)}
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

  /* Repos list */
  .repos-list {
    max-width: 800px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 32px;
  }

  .repo-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .repo-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 4px;
  }

  :global(.repo-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .repo-name {
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .repo-path {
    font-size: var(--size-xs);
    color: var(--text-faint);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  .branches-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* New branch button at bottom */
  .new-branch-section {
    display: flex;
    justify-content: center;
    padding-top: 8px;
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
