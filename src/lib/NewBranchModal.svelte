<!--
  NewBranchModal.svelte - Create a new branch with worktree

  Two-step flow:
  1. Pick a repository (with search)
  2. Enter branch name

  The branch is created with an isolated worktree, defaulting to the
  repository's default branch (e.g., origin/main) as the base.
  The base branch can be changed later from the BranchCard.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    X,
    Folder,
    GitBranch,
    ChevronRight,
    Search,
    Loader2,
    ArrowLeft,
    ChevronsUpDown,
    Check,
  } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import * as branchService from './services/branch';
  import { listDirectory, getHomeDir, searchDirectories, type DirEntry } from './services/files';
  import { listRefs } from './services/git';

  /** Info about a branch being created (for showing a placeholder) */
  export interface PendingBranch {
    repoPath: string;
    branchName: string;
    baseBranch: string;
  }

  interface Props {
    onCreating: (pending: PendingBranch) => void;
    onCreated: (branch: Branch) => void;
    onCreateFailed: (pending: PendingBranch, error: string) => void;
    onClose: () => void;
  }

  let { onCreating, onCreated, onCreateFailed, onClose }: Props = $props();

  // State
  type Step = 'repo' | 'name';
  let step = $state<Step>('repo');
  let selectedRepo = $state<string | null>(null);
  let branchName = $state('');

  // Repo picker state
  let query = $state('');
  let currentDir = $state('');
  let homeDir = $state('');
  let entries = $state<DirEntry[]>([]);
  let searchResults = $state<DirEntry[]>([]);
  let loading = $state(false);
  let searching = $state(false);
  let selectedIndex = $state(0);
  let searchTimeout: ReturnType<typeof setTimeout> | null = null;

  // Default branch (detected when repo is selected)
  let detectedDefaultBranch = $state<string | null>(null);

  // Base branch picker state
  let selectedBaseBranch = $state<string | null>(null);
  let showBasePicker = $state(false);
  let availableBranches = $state<string[]>([]);
  let baseSearchQuery = $state('');
  let baseSelectedIndex = $state(0);

  let inputEl: HTMLInputElement | null = $state(null);
  let branchInputEl: HTMLInputElement | null = $state(null);
  let baseSearchEl: HTMLInputElement | null = $state(null);

  let isSearching = $derived(query.length >= 2);

  // Effective base branch (selected or detected default)
  let effectiveBaseBranch = $derived(selectedBaseBranch ?? detectedDefaultBranch ?? 'main');

  // Filtered branches for base picker
  let filteredBranches = $derived.by(() => {
    if (!baseSearchQuery) return availableBranches;
    const q = baseSearchQuery.toLowerCase();
    return availableBranches.filter((b) => b.toLowerCase().includes(q));
  });

  // Initialize
  onMount(async () => {
    const dir = await getHomeDir();
    homeDir = dir;
    currentDir = dir;
  });

  // Focus appropriate input
  $effect(() => {
    if (step === 'repo' && inputEl) {
      inputEl.focus();
    } else if (step === 'name' && branchInputEl && !showBasePicker) {
      branchInputEl.focus();
    }
  });

  // Focus base search when picker opens
  $effect(() => {
    if (showBasePicker && baseSearchEl) {
      baseSearchEl.focus();
    }
  });

  // Load directory when currentDir changes
  $effect(() => {
    if (currentDir && !isSearching) {
      loadDirectory(currentDir);
    }
  });

  // Debounced search
  $effect(() => {
    if (searchTimeout) clearTimeout(searchTimeout);

    if (!query || query.length < 2) {
      searchResults = [];
      searching = false;
      return;
    }

    searching = true;
    searchTimeout = setTimeout(async () => {
      try {
        const depth = currentDir === homeDir ? 4 : 3;
        const results = await searchDirectories(currentDir, query, depth, 20);
        searchResults = results;
        selectedIndex = 0;
      } catch (e) {
        console.error('Search failed:', e);
        searchResults = [];
      } finally {
        searching = false;
      }
    }, 150);
  });

  async function loadDirectory(path: string) {
    loading = true;
    try {
      const allEntries = await listDirectory(path);
      entries = allEntries.filter((e) => e.isDir);
      selectedIndex = 0;
    } catch (e) {
      entries = [];
    } finally {
      loading = false;
    }
  }

  // Get display items based on mode
  let displayItems = $derived.by(() => {
    if (isSearching) {
      return searchResults;
    }
    return entries;
  });

  async function selectRepo(path: string) {
    selectedRepo = path;

    // Detect the default branch for display
    try {
      detectedDefaultBranch = await branchService.detectDefaultBranch(path);
    } catch (e) {
      console.error('Failed to detect default branch:', e);
      detectedDefaultBranch = 'main';
    }

    // Load available branches for the picker
    try {
      const refs = await listRefs(path);
      availableBranches = refs;
    } catch (e) {
      console.error('Failed to load branches:', e);
      availableBranches = [];
    }

    step = 'name';
  }

  function handleEntryClick(entry: DirEntry) {
    if (entry.isRepo) {
      selectRepo(entry.path);
    } else {
      // Navigate into directory
      currentDir = entry.path;
      query = '';
    }
  }

  function goBack() {
    if (showBasePicker) {
      showBasePicker = false;
      baseSearchQuery = '';
      baseSelectedIndex = 0;
    } else if (step === 'name') {
      step = 'repo';
      selectedRepo = null;
      detectedDefaultBranch = null;
      selectedBaseBranch = null;
      branchName = '';
    }
  }

  function goUp() {
    if (currentDir && currentDir !== '/') {
      const parent = currentDir.split('/').slice(0, -1).join('/') || '/';
      currentDir = parent;
      query = '';
    }
  }

  function goHome() {
    if (homeDir) {
      currentDir = homeDir;
      query = '';
    }
  }

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
    if (!selectedRepo || !branchName.trim()) return;

    const pending: PendingBranch = {
      repoPath: selectedRepo,
      branchName: branchName.trim(),
      baseBranch: effectiveBaseBranch,
    };

    // Notify parent immediately so it can show a placeholder
    onCreating(pending);

    try {
      // Pass selected base branch or undefined to use detected default
      const branch = await branchService.createBranch(
        selectedRepo,
        branchName.trim(),
        selectedBaseBranch ?? undefined
      );
      onCreated(branch);
    } catch (e) {
      const errorMsg = e instanceof Error ? e.message : String(e);
      onCreateFailed(pending, errorMsg);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (showBasePicker) {
        showBasePicker = false;
        baseSearchQuery = '';
      } else if (step === 'name') {
        goBack();
      } else {
        onClose();
      }
      return;
    }

    if (step === 'repo') {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        selectedIndex = Math.min(selectedIndex + 1, displayItems.length - 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        selectedIndex = Math.max(selectedIndex - 1, 0);
      } else if (e.key === 'Enter' && displayItems.length > 0) {
        e.preventDefault();
        handleEntryClick(displayItems[selectedIndex]);
      } else if (e.key === 'Backspace' && !query) {
        e.preventDefault();
        goUp();
      }
    } else if (step === 'name') {
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
  }

  // Extract repo name from path
  function repoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
  }

  // Format branch name for display (strip origin/ prefix)
  function formatBranchName(name: string): string {
    return name.replace(/^origin\//, '');
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
      {#if step === 'name'}
        <button class="back-button" onclick={goBack}>
          <ArrowLeft size={16} />
        </button>
      {/if}
      <h2>
        {#if step === 'repo'}
          Select Repository
        {:else}
          New Branch
        {/if}
      </h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    {#if step === 'repo'}
      <!-- Repository picker -->
      <div class="search-container">
        <Search size={16} class="search-icon" />
        <input
          bind:this={inputEl}
          bind:value={query}
          type="text"
          placeholder="Search repositories..."
          class="search-input"
        />
        {#if searching}
          <Loader2 size={16} class="spinner" />
        {/if}
      </div>

      <div class="breadcrumb">
        <button class="breadcrumb-home" onclick={goHome}>~</button>
        {#if currentDir && currentDir !== homeDir}
          <ChevronRight size={12} />
          <span class="breadcrumb-path">
            {currentDir.replace(homeDir, '').replace(/^\//, '')}
          </span>
        {/if}
      </div>

      <div class="entries-list">
        {#if loading && !isSearching}
          <div class="loading">
            <Loader2 size={16} class="spinner" />
            <span>Loading...</span>
          </div>
        {:else if displayItems.length === 0}
          <div class="empty">
            {isSearching ? 'No repositories found' : 'No folders'}
          </div>
        {:else}
          {#each displayItems as entry, index (entry.path)}
            <button
              class="entry"
              class:selected={index === selectedIndex}
              class:repo={entry.isRepo}
              onclick={() => handleEntryClick(entry)}
            >
              {#if entry.isRepo}
                <GitBranch size={16} class="entry-icon repo-icon" />
              {:else}
                <Folder size={16} class="entry-icon" />
              {/if}
              <span class="entry-name">{entry.name}</span>
              {#if !entry.isRepo}
                <ChevronRight size={14} class="entry-chevron" />
              {/if}
            </button>
          {/each}
        {/if}
      </div>
    {:else}
      <!-- Branch name input -->
      <div class="name-step">
        <div class="selected-info">
          <div class="info-row">
            <GitBranch size={14} />
            <span class="info-label">Repository:</span>
            <span class="info-value">{repoName(selectedRepo ?? '')}</span>
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
            <label for="branch-name">Branch name</label>
            <input
              bind:this={branchInputEl}
              bind:value={branchName}
              id="branch-name"
              type="text"
              placeholder="feature/my-feature"
              class="branch-input"
              autocomplete="off"
              autocorrect="off"
              autocapitalize="off"
              spellcheck="false"
            />
          </div>

          <div class="actions">
            <button class="cancel-button" onclick={goBack}>Cancel</button>
            <button class="create-button" onclick={handleCreate} disabled={!branchName.trim()}>
              Create Branch
            </button>
          </div>
        {/if}
      </div>
    {/if}
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
    width: 500px;
    max-width: 90vw;
    max-height: 70vh;
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

  .back-button,
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

  .back-button:hover,
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

  /* Breadcrumb */
  .breadcrumb {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 16px;
    font-size: var(--size-sm);
    color: var(--text-muted);
    border-bottom: 1px solid var(--border-subtle);
  }

  .breadcrumb-home {
    padding: 2px 6px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', monospace;
    cursor: pointer;
  }

  .breadcrumb-home:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .breadcrumb-path {
    font-family: 'SF Mono', 'Menlo', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Entries list */
  .entries-list {
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

  .entry {
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

  .entry:hover,
  .entry.selected {
    background-color: var(--bg-hover);
  }

  .entry.repo {
    color: var(--ui-accent);
  }

  :global(.entry-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  :global(.repo-icon) {
    color: var(--status-renamed);
  }

  .entry-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.entry-chevron) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  /* Name step */
  .name-step {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
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
    background-color: var(--bg-selected);
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
