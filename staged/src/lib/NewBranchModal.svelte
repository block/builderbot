<!--
  NewBranchModal.svelte - Create a new branch (local worktree or remote Blox workspace)

  Two modes toggled by a segmented control:
  - Local: branch name + base branch picker → creates git worktree
  - Remote: branch name → starts a Blox workspace
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, GitBranch, Search, ChevronsUpDown, Check, Monitor, Cloud } from 'lucide-svelte';
  import Spinner from './Spinner.svelte';
  import type { Branch, Project, BranchType } from './types';
  import * as commands from './commands';
  import { runPrerunActions } from './services/actions';

  interface Props {
    project: Project;
    onCreated: (branch: Branch) => void;
    onClose: () => void;
  }

  let { project, onCreated, onClose }: Props = $props();

  // Branch type toggle
  let branchType = $state<BranchType>('local');

  // State
  let branchTitle = $state('');
  let creating = $state(false);
  let error = $state<string | null>(null);

  // Default branch (detected on mount)
  let detectedDefaultBranch = $state<string | null>(null);
  let selectedBaseBranch = $state<string | null>(null);

  // Base branch picker
  let showBasePicker = $state(false);
  let availableBranches = $state<string[]>([]);
  let baseSearchQuery = $state('');
  let baseSelectedIndex = $state(0);

  let branchInputEl: HTMLInputElement | null = $state(null);
  let baseSearchEl: HTMLInputElement | null = $state(null);

  let effectiveBaseBranch = $derived(selectedBaseBranch ?? detectedDefaultBranch ?? 'main');

  let filteredBranches = $derived.by(() => {
    if (!baseSearchQuery) return availableBranches;
    const q = baseSearchQuery.toLowerCase();
    return availableBranches.filter((b) => b.toLowerCase().includes(q));
  });

  /**
   * Sanitize a branch title into a valid git branch name.
   */
  function sanitizeBranchName(title: string): string {
    return title
      .toLowerCase()
      .replace(/[\s_]+/g, '-')
      .replace(/[~^:?*\[\]\\@{}"'`!#$%&()|<>=+;,]/g, '')
      .replace(/[-.]+/g, '-')
      .replace(/^[-.]+|[-.]+$/g, '');
  }

  let branchName = $derived(sanitizeBranchName(branchTitle));

  /** Generate a workspace name from the branch name. */
  function workspaceName(name: string): string {
    if (!name) return '';
    // Prefix with repo name for uniqueness
    const repo = repoName(project.repoPath);
    return `${repo}-${name}`;
  }

  onMount(async () => {
    // Detect default branch
    try {
      detectedDefaultBranch = await commands.detectDefaultBranch(project.repoPath);
    } catch {
      detectedDefaultBranch = 'main';
    }

    // Load available branches
    try {
      const refs = await commands.listGitBranches(project.repoPath);
      availableBranches = refs.map((r) => r.name);
    } catch {
      availableBranches = [];
    }
  });

  // Focus branch input
  $effect(() => {
    if (branchInputEl && !showBasePicker) {
      branchInputEl.focus();
    }
  });

  // Focus base search when picker opens
  $effect(() => {
    if (showBasePicker && baseSearchEl) {
      baseSearchEl.focus();
    }
  });

  function toggleBasePicker() {
    showBasePicker = !showBasePicker;
    baseSearchQuery = '';
    baseSelectedIndex = 0;
  }

  function selectBaseBranch(branch: string) {
    selectedBaseBranch = branch;
    showBasePicker = false;
    baseSearchQuery = '';
    baseSelectedIndex = 0;
  }

  async function handleCreate() {
    if (!branchName.trim() || creating) return;

    creating = true;
    error = null;

    try {
      if (branchType === 'local') {
        const baseBranch = selectedBaseBranch ?? undefined;
        const branch = await commands.createBranch(project.id, branchName.trim(), baseBranch);

        // Capture values before modal closes
        const branchId = branch.id;
        const projectId = project.id;

        onCreated(branch);

        // Wait for BranchCard to be created and listeners to be set up
        // The BranchCard sets up listeners at module level, but the component needs to be created first
        setTimeout(() => {
          runPrerunActions(branchId, projectId).catch((e) => {
            console.error('[NewBranchModal] Failed to run prerun actions:', e);
          });
        }, 150);
      } else {
        const wsName = workspaceName(branchName.trim());
        const branch = await commands.createRemoteBranch(
          project.id,
          branchName.trim(),
          wsName,
          selectedBaseBranch ?? undefined,
          undefined,
          project.repoPath
        );
        onCreated(branch);
      }
    } catch (e) {
      if (typeof e === 'string') {
        error = e;
      } else if (e instanceof Error) {
        error = e.message;
      } else {
        error = String(e);
      }
      creating = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (showBasePicker) {
        showBasePicker = false;
        baseSearchQuery = '';
      } else {
        onClose();
      }
      return;
    }

    if (showBasePicker) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        baseSelectedIndex = Math.min(baseSelectedIndex + 1, filteredBranches.length - 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        baseSelectedIndex = Math.max(baseSelectedIndex - 1, 0);
      } else if (e.key === 'Enter' && filteredBranches.length > 0) {
        e.preventDefault();
        selectBaseBranch(filteredBranches[baseSelectedIndex]);
      }
    } else {
      if (e.key === 'Enter' && branchName.trim()) {
        e.preventDefault();
        handleCreate();
      }
    }
  }

  function formatBranchName(name: string): string {
    return name.replace(/^origin\//, '');
  }

  function repoName(path: string): string {
    return path.split('/').pop() || path;
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <div class="modal-header">
      <h2>New Branch</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="modal-body">
      <!-- Branch type toggle -->
      <div class="type-toggle">
        <button
          class="toggle-option"
          class:active={branchType === 'local'}
          onclick={() => (branchType = 'local')}
        >
          <Monitor size={14} />
          Local
        </button>
        <button
          class="toggle-option"
          class:active={branchType === 'remote'}
          onclick={() => (branchType = 'remote')}
        >
          <Cloud size={14} />
          Remote
        </button>
      </div>

      <div class="selected-info">
        <div class="info-row">
          <GitBranch size={14} />
          <span class="info-label">Repository:</span>
          <span class="info-value">{repoName(project.repoPath)}</span>
        </div>
        <button class="info-row base-row" onclick={toggleBasePicker}>
          <GitBranch size={14} class="base-icon" />
          <span class="info-label">Base:</span>
          <span class="info-value">{formatBranchName(effectiveBaseBranch)}</span>
          <ChevronsUpDown size={12} class="base-chevron" />
        </button>
      </div>

      {#if showBasePicker}
        <!-- Base branch picker -->
        <div class="base-picker">
          <div class="base-search-container">
            <Search size={14} class="search-icon" />
            <input
              bind:this={baseSearchEl}
              bind:value={baseSearchQuery}
              type="text"
              placeholder="Search branches..."
              class="base-search-input"
            />
          </div>
          <div class="base-list">
            {#each filteredBranches as branch, index (branch)}
              <button
                class="base-item"
                class:selected={index === baseSelectedIndex}
                onclick={() => selectBaseBranch(branch)}
              >
                <span class="base-item-name">{branch}</span>
                {#if branch === effectiveBaseBranch}
                  <Check size={14} class="check-icon" />
                {/if}
              </button>
            {/each}
            {#if filteredBranches.length === 0}
              <div class="base-empty">No branches found</div>
            {/if}
          </div>
        </div>
      {:else}
        <div class="input-group">
          <label for="branch-title">Branch name</label>
          <input
            bind:this={branchInputEl}
            bind:value={branchTitle}
            id="branch-title"
            type="text"
            placeholder={branchType === 'local' ? 'Fix login issue' : 'Add user auth'}
            class="branch-input"
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck="false"
          />
          {#if branchTitle && branchName !== branchTitle.toLowerCase()}
            <div class="branch-preview">
              <GitBranch size={12} />
              <span>{branchName || '...'}</span>
            </div>
          {/if}
          {#if branchType === 'remote' && branchName}
            <div class="workspace-preview">
              <Cloud size={12} />
              <span>Workspace: {workspaceName(branchName)}</span>
            </div>
          {/if}
        </div>

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="actions">
          <button class="cancel-button" onclick={onClose}>Cancel</button>
          <button class="create-button" onclick={handleCreate} disabled={!branchName || creating}>
            {#if creating}
              <Spinner size={14} />
              Creating...
            {:else}
              Create Branch
            {/if}
          </button>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 10vh;
    z-index: 1000;
  }

  .modal {
    width: 460px;
    max-width: 90vw;
    background-color: var(--bg-primary);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    flex: 1;
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

  .modal-body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  /* Branch type toggle */
  .type-toggle {
    display: flex;
    gap: 2px;
    padding: 3px;
    background-color: var(--bg-hover);
    border-radius: 8px;
  }

  .toggle-option {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 7px 12px;
    background: transparent;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .toggle-option:hover:not(.active) {
    color: var(--text-primary);
  }

  .toggle-option.active {
    background-color: var(--bg-primary);
    color: var(--text-primary);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
  }

  .toggle-option :global(svg) {
    flex-shrink: 0;
  }

  .selected-info {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 6px;
  }

  .info-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-sm);
  }

  .info-row :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  :global(.base-icon) {
    color: var(--text-muted) !important;
  }

  .info-label {
    color: var(--text-muted);
  }

  .info-value {
    color: var(--text-primary);
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  .base-row {
    background: transparent;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    margin: -4px -6px;
    padding: 4px 6px;
    transition: background-color 0.15s;
  }

  .base-row:hover {
    background-color: var(--bg-primary);
  }

  :global(.base-chevron) {
    color: var(--text-faint);
    margin-left: auto;
  }

  .base-row:hover :global(.base-chevron) {
    color: var(--text-muted);
  }

  /* Base branch picker */
  .base-picker {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    overflow: hidden;
  }

  .base-search-container {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .base-search-input {
    flex: 1;
    padding: 4px 0;
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .base-search-input::placeholder {
    color: var(--text-faint);
  }

  .base-list {
    max-height: 200px;
    overflow-y: auto;
    padding: 4px;
  }

  .base-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .base-item:hover,
  .base-item.selected {
    background-color: var(--bg-hover);
  }

  .base-item-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  :global(.check-icon) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .base-empty {
    padding: 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .input-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .input-group label {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .branch-input {
    padding: 10px 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: var(--size-md);
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s;
  }

  .branch-input:focus {
    border-color: var(--ui-accent);
  }

  .branch-input::placeholder {
    color: var(--text-faint);
  }

  .branch-preview {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .branch-preview :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .branch-preview span {
    font-family: 'SF Mono', 'Menlo', monospace;
    color: var(--text-muted);
  }

  .workspace-preview {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 2px;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .workspace-preview :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .workspace-preview span {
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  .error-message {
    padding: 10px 12px;
    background-color: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }

  .cancel-button {
    padding: 8px 16px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s;
  }

  .cancel-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .create-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .create-button:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .create-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
