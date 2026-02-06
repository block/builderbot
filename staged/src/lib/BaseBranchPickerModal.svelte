<!--
  BaseBranchPickerModal.svelte - Modal to change a branch's base branch

  Shows a searchable list of local and remote branches to select as the new base.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, Search, Loader2, GitBranch, Globe, Check } from 'lucide-svelte';
  import type { Branch, BranchRef } from './services/branch';
  import * as branchService from './services/branch';

  interface Props {
    branch: Branch;
    onSelected: (baseBranch: string) => void;
    onClose: () => void;
  }

  let { branch, onSelected, onClose }: Props = $props();

  // State
  let query = $state('');
  let gitBranches = $state<BranchRef[]>([]);
  let loading = $state(true);
  let updating = $state(false);
  let error = $state<string | null>(null);
  let selectedIndex = $state(0);

  let inputEl: HTMLInputElement | null = $state(null);

  // Filter branches by query
  let filteredBranches = $derived.by(() => {
    if (!query) return gitBranches;
    const q = query.toLowerCase();
    return gitBranches.filter((b) => b.name.toLowerCase().includes(q));
  });

  // Load branches on mount
  onMount(async () => {
    try {
      const branches = await branchService.listGitBranches(branch.repoPath);
      gitBranches = branches;
      // Find current base branch index
      const currentIndex = branches.findIndex((b) => b.name === branch.baseBranch);
      selectedIndex = currentIndex >= 0 ? currentIndex : 0;
    } catch (e) {
      console.error('Failed to load branches:', e);
      error = 'Failed to load branches';
    } finally {
      loading = false;
    }
  });

  // Focus input on mount
  $effect(() => {
    if (inputEl && !loading) {
      inputEl.focus();
    }
  });

  // Reset selected index when filter changes
  $effect(() => {
    if (filteredBranches.length > 0) {
      selectedIndex = Math.min(selectedIndex, filteredBranches.length - 1);
    }
  });

  async function selectBranch(branchName: string) {
    if (branchName === branch.baseBranch) {
      onClose();
      return;
    }

    updating = true;
    error = null;

    try {
      await branchService.updateBranchBase(branch.id, branchName);
      onSelected(branchName);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      updating = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, filteredBranches.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
    } else if (e.key === 'Enter' && filteredBranches.length > 0) {
      e.preventDefault();
      selectBranch(filteredBranches[selectedIndex].name);
    }
  }
</script>

<div class="modal-backdrop" role="button" tabindex="-1" onclick={onClose} onkeydown={handleKeydown}>
  <div
    class="modal"
    role="dialog"
    tabindex="-1"
    onkeydown={() => {}}
    onclick={(e) => e.stopPropagation()}
  >
    <div class="modal-header">
      <h2>Change Base Branch</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="search-container">
      <Search size={16} class="search-icon" />
      <input
        bind:this={inputEl}
        bind:value={query}
        type="text"
        placeholder="Filter branches..."
        class="search-input"
        disabled={loading || updating}
      />
    </div>

    <div class="current-info">
      <span class="current-label">Current:</span>
      <span class="current-value">{branch.baseBranch}</span>
    </div>

    {#if error}
      <div class="error">{error}</div>
    {/if}

    <div class="branches-list">
      {#if loading}
        <div class="loading">
          <Loader2 size={16} class="spinner" />
          <span>Loading branches...</span>
        </div>
      {:else if updating}
        <div class="loading">
          <Loader2 size={16} class="spinner" />
          <span>Updating...</span>
        </div>
      {:else if filteredBranches.length === 0}
        <div class="empty">
          {query ? 'No matching branches' : 'No branches found'}
        </div>
      {:else}
        {#each filteredBranches as branchRef, index (branchRef.name)}
          <button
            class="branch-item"
            class:selected={index === selectedIndex}
            class:current={branchRef.name === branch.baseBranch}
            onclick={() => selectBranch(branchRef.name)}
          >
            {#if branchRef.isRemote}
              <Globe size={16} class="branch-item-icon remote-icon" />
            {:else}
              <GitBranch size={16} class="branch-item-icon local-icon" />
            {/if}
            <span class="branch-item-name">{branchRef.name}</span>
            {#if branchRef.name === branch.baseBranch}
              <Check size={14} class="current-check" />
            {/if}
          </button>
        {/each}
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 10vh;
    z-index: 1000;
  }

  .modal {
    width: 400px;
    max-width: 90vw;
    max-height: 60vh;
    background-color: var(--bg-primary);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .close-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Search */
  .search-container {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .search-input {
    flex: 1;
    padding: 8px 0;
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-md);
    color: var(--text-primary);
  }

  .search-input::placeholder {
    color: var(--text-faint);
  }

  .search-input:disabled {
    opacity: 0.5;
  }

  /* Current info */
  .current-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    font-size: var(--size-sm);
    border-bottom: 1px solid var(--border-subtle);
  }

  .current-label {
    color: var(--text-muted);
  }

  .current-value {
    color: var(--text-primary);
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  /* Error */
  .error {
    margin: 8px 16px;
    padding: 8px 12px;
    background-color: var(--ui-danger-bg);
    border-radius: 6px;
    font-size: var(--size-sm);
    color: var(--ui-danger);
  }

  /* Branches list */
  .branches-list {
    flex: 1;
    overflow-y: auto;
    padding: 8px;
  }

  .loading,
  .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 24px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .branch-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 12px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .branch-item:hover,
  .branch-item.selected {
    background-color: var(--bg-hover);
  }

  .branch-item.current {
    background-color: var(--bg-selected);
  }

  :global(.branch-item-icon) {
    flex-shrink: 0;
  }

  :global(.remote-icon) {
    color: var(--text-muted);
  }

  :global(.local-icon) {
    color: var(--status-renamed);
  }

  .branch-item-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  :global(.current-check) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  /* Spinner */
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
