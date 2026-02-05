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
  import { Plus, Sparkles, Folder } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import * as branchService from './services/branch';
  import { listenToSessionStatus, type SessionStatusEvent } from './services/ai';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import BranchCard from './BranchCard.svelte';
  import NewBranchModal from './NewBranchModal.svelte';
  import { DiffSpec } from './types';

  interface Props {
    onViewDiff?: (repoPath: string, spec: DiffSpec, label: string) => void;
  }

  let { onViewDiff }: Props = $props();

  // State
  let branches = $state<Branch[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshKey = $state(0);

  // Event listener cleanup
  let unlistenStatus: UnlistenFn | null = null;

  // Modal state
  let showNewBranchModal = $state(false);

  // Group branches by repo path
  let branchesByRepo = $derived.by(() => {
    const grouped = new Map<string, Branch[]>();
    for (const branch of branches) {
      const existing = grouped.get(branch.repoPath) || [];
      existing.push(branch);
      grouped.set(branch.repoPath, existing);
    }
    return grouped;
  });

  // Extract repo name from path
  function repoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
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

  async function handleBranchCreated(branch: Branch) {
    branches = [...branches, branch];
    showNewBranchModal = false;
  }

  async function handleDeleteBranch(branchId: string) {
    const branch = branches.find((b) => b.id === branchId);
    if (!branch) return;

    const confirmed = await confirm(`Delete branch "${branch.branchName}" and its worktree?`);
    if (!confirmed) return;

    try {
      await branchService.deleteBranch(branchId);
      branches = branches.filter((b) => b.id !== branchId);
    } catch (e) {
      console.error('Failed to delete branch:', e);
    }
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
    {:else if branches.length === 0}
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
        {#each [...branchesByRepo.entries()] as [repoPath, repoBranches] (repoPath)}
          <div class="repo-section">
            <div class="repo-header">
              <Folder size={14} class="repo-icon" />
              <span class="repo-name">{repoName(repoPath)}</span>
              <span class="repo-path">{repoPath}</span>
            </div>
            <div class="branches-list">
              {#each repoBranches as branch (branch.id)}
                <BranchCard
                  {branch}
                  {refreshKey}
                  onViewDiff={() => handleViewDiff(branch)}
                  onViewCommitDiff={(sha) => handleViewCommitDiff(branch, sha)}
                  onDelete={() => handleDeleteBranch(branch.id)}
                />
              {/each}
            </div>
          </div>
        {/each}

        <!-- New branch button at the bottom -->
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
  <NewBranchModal onCreated={handleBranchCreated} onClose={() => (showNewBranchModal = false)} />
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
    padding: 24px;
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

  /* New branch section */
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
</style>
