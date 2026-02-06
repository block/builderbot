<!--
  NewProjectModal.svelte - Create a new project from a repository

  Two-step flow:
  1. Pick a repository (with search)
  2. Configure project name and optional subpath
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, Folder, GitBranch, ChevronRight, Search, Loader2, ArrowLeft } from 'lucide-svelte';
  import type { GitProject } from './services/branch';
  import * as branchService from './services/branch';
  import { listDirectory, getHomeDir, searchDirectories, type DirEntry } from './services/files';

  interface Props {
    onCreated: (project: GitProject) => void;
    onClose: () => void;
    onDetecting?: (projectId: string, isDetecting: boolean) => void;
  }

  let { onCreated, onClose, onDetecting }: Props = $props();

  // State
  type Step = 'repo' | 'config';
  let step = $state<Step>('repo');
  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);

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

  let inputEl: HTMLInputElement | null = $state(null);
  let subpathInputEl: HTMLInputElement | null = $state(null);

  // Subpath suggestions
  let subpathInputFocused = $state(false);
  let showSubpathDropdown = $state(false);
  let subpathSuggestions = $state<DirEntry[]>([]);
  let subpathSelectedIndex = $state(0);
  let subpathSearchTimeout: ReturnType<typeof setTimeout> | null = null;

  let isSearching = $derived(query.length >= 2);

  // Initialize
  onMount(async () => {
    const dir = await getHomeDir();
    homeDir = dir;
    currentDir = dir;
  });

  // Focus search input when on repo step
  $effect(() => {
    if (step === 'repo' && inputEl) {
      inputEl.focus();
    }
  });

  // Load directory when currentDir changes (and not in search mode)
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

  let displayItems = $derived.by(() => {
    if (isSearching) return searchResults;
    return entries;
  });

  // Split the current subpath value into a directory to list and a partial segment to filter by.
  // e.g. "packages/fro" → list repo/packages/, filter by "fro"
  function getSubpathContext(): { dir: string; query: string } {
    if (!selectedRepo) return { dir: '', query: '' };
    const trimmed = subpath.replace(/^\/+/, '');
    if (!trimmed || trimmed.endsWith('/')) {
      const parentRel = trimmed.replace(/\/+$/, '');
      return {
        dir: parentRel ? selectedRepo + '/' + parentRel : selectedRepo,
        query: '',
      };
    }
    const lastSlash = trimmed.lastIndexOf('/');
    if (lastSlash === -1) {
      return { dir: selectedRepo, query: trimmed };
    }
    return {
      dir: selectedRepo + '/' + trimmed.slice(0, lastSlash),
      query: trimmed.slice(lastSlash + 1),
    };
  }

  // Reload the directory listing whenever subpath changes while dropdown is visible
  $effect(() => {
    if (!selectedRepo || step !== 'config' || !showSubpathDropdown) {
      subpathSuggestions = [];
      return;
    }

    if (subpathSearchTimeout) clearTimeout(subpathSearchTimeout);

    const { dir } = getSubpathContext();
    subpathSearchTimeout = setTimeout(async () => {
      try {
        const allEntries = await listDirectory(dir);
        subpathSuggestions = allEntries.filter((e) => e.isDir);
        subpathSelectedIndex = 0;
      } catch {
        subpathSuggestions = [];
      }
    }, 100);
  });

  // Filter the loaded listing by the partial trailing segment
  let filteredSubpathSuggestions = $derived.by(() => {
    const { query: q } = getSubpathContext();
    if (!q) return subpathSuggestions;
    const lower = q.toLowerCase();
    return subpathSuggestions.filter((e) => e.name.toLowerCase().includes(lower));
  });

  function selectSubpathSuggestion(entry: DirEntry) {
    // Fill the full relative path and append "/" so the next level loads immediately
    subpath = entry.path.replace(selectedRepo! + '/', '') + '/';
    subpathSelectedIndex = 0;
    // Hide the dropdown after selection
    showSubpathDropdown = false;
  }

  // Arrow/Enter navigation within the suggestion dropdown — lives on the input itself
  // so it fires regardless of how outer containers handle propagation.
  function handleSubpathKeydown(e: KeyboardEvent) {
    if (filteredSubpathSuggestions.length === 0) return;
    // Let Meta/Ctrl+Enter fall through to the modal's "create" handler
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) return;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      subpathSelectedIndex = Math.min(
        subpathSelectedIndex + 1,
        filteredSubpathSuggestions.length - 1
      );
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      subpathSelectedIndex = Math.max(subpathSelectedIndex - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      selectSubpathSuggestion(filteredSubpathSuggestions[subpathSelectedIndex]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      showSubpathDropdown = false;
      subpathInputFocused = false;
    }
  }

  function handleSubpathInput() {
    // Show dropdown when user starts typing
    showSubpathDropdown = true;
  }

  function handleSubpathClick() {
    // Only open dropdown when clicking directly in the input field
    showSubpathDropdown = true;
  }

  function handleSubpathFocus() {
    subpathInputFocused = true;
    // Don't open dropdown on focus - wait for click or typing
    // This prevents the dropdown from opening when clicking the label
  }

  function handleSubpathBlur(e: FocusEvent) {
    subpathInputFocused = false;
    // Only close if focus is moving outside the dropdown
    // Check if the related target (where focus is going) is inside the dropdown
    const relatedTarget = e.relatedTarget as HTMLElement | null;
    const container = (e.target as HTMLElement)?.closest('.subpath-input-container');
    if (!relatedTarget || !container?.contains(relatedTarget)) {
      showSubpathDropdown = false;
    }
  }

  function selectRepo(path: string) {
    selectedRepo = path;
    step = 'config';
  }

  function handleEntryClick(entry: DirEntry) {
    if (entry.isRepo) {
      selectRepo(entry.path);
    } else {
      currentDir = entry.path;
      query = '';
    }
  }

  function goBack() {
    if (step === 'config') {
      step = 'repo';
      selectedRepo = null;
      subpath = '';
      error = null;
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

  async function handleCreate() {
    if (!selectedRepo || saving) return;

    saving = true;
    error = null;

    try {
      const normalizedSubpath = subpath.trim().replace(/^\/+|\/+$/g, '') || undefined;
      const project = await branchService.createGitProject(selectedRepo, normalizedSubpath);

      // Detect and save actions in the background (don't block on success)
      detectAndSaveActions(project.id).catch((e) => {
        console.warn('Failed to auto-detect actions for new project:', e);
        // Silent failure - user can still detect actions manually later
      });

      onCreated(project);
    } catch (e) {
      // Extract error message from various error formats
      let errorMessage: string;
      if (typeof e === 'string') {
        errorMessage = e;
      } else if (e instanceof Error) {
        errorMessage = e.message;
      } else if (e && typeof e === 'object' && 'message' in e) {
        errorMessage = String((e as any).message);
      } else {
        errorMessage = String(e);
      }

      error = errorMessage;
      saving = false;
    }
  }

  async function detectAndSaveActions(projectId: string) {
    try {
      // Notify that detection is starting
      onDetecting?.(projectId, true);

      // Detect actions using AI
      const suggested = await branchService.detectProjectActions(projectId);

      // Save each suggested action
      let sortOrder = 1;
      for (const suggestion of suggested) {
        await branchService.createProjectAction(
          projectId,
          suggestion.name,
          suggestion.command,
          suggestion.actionType,
          sortOrder++,
          suggestion.autoCommit
        );
      }
    } finally {
      // Notify that detection is complete (success or failure)
      onDetecting?.(projectId, false);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (step === 'config') {
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
    } else if (step === 'config') {
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        handleCreate();
      }
    }
  }

  function repoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
  }

  function handleBackdropClick(event: MouseEvent) {
    // Only close if clicking directly on the backdrop, not on children
    // This prevents accidental closes during text selection
    if (event.target === event.currentTarget) {
      onClose();
    }
  }
</script>

<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={handleKeydown}
>
  <div class="modal">
    <div class="modal-header">
      {#if step === 'config'}
        <button class="back-button" onclick={goBack}>
          <ArrowLeft size={16} />
        </button>
      {/if}
      <h2>
        {#if step === 'repo'}
          New Project
        {:else}
          Configure Project
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
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck="false"
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
      <!-- Project configuration -->
      <div class="config-step">
        <div class="repo-info">
          <GitBranch size={14} class="repo-info-icon" />
          <span class="repo-info-label">Repository:</span>
          <span class="repo-info-value">{repoName(selectedRepo ?? '')}</span>
        </div>

        <div class="form-group">
          <label for="project-subpath">Subpath <span class="optional-label">(optional)</span></label
          >
          <div class="subpath-input-container">
            <input
              bind:this={subpathInputEl}
              bind:value={subpath}
              id="project-subpath"
              type="text"
              placeholder="e.g., packages/frontend"
              disabled={saving}
              autocomplete="off"
              autocorrect="off"
              autocapitalize="off"
              spellcheck="false"
              onclick={handleSubpathClick}
              onfocus={handleSubpathFocus}
              onblur={handleSubpathBlur}
              oninput={handleSubpathInput}
              onkeydown={handleSubpathKeydown}
            />
            {#if showSubpathDropdown && filteredSubpathSuggestions.length > 0}
              <div class="subpath-suggestions">
                {#each filteredSubpathSuggestions as entry, index (entry.path)}
                  <button
                    class="subpath-suggestion"
                    class:selected={index === subpathSelectedIndex}
                    onmousedown={(e) => {
                      e.preventDefault();
                      selectSubpathSuggestion(entry);
                    }}
                  >
                    <Folder size={14} class="suggestion-icon" />
                    <span class="suggestion-name">{entry.name}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
          <span class="help-text">
            For monorepos: subdirectory to use as working directory for AI sessions
          </span>
        </div>

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="actions">
          <button class="cancel-button" onclick={goBack} disabled={saving}>Cancel</button>
          <button class="create-button" onclick={handleCreate} disabled={saving}>
            {saving ? 'Creating...' : 'Create Project'}
          </button>
        </div>
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
    /* Note: no overflow:hidden here - entries-list handles its own scrolling,
       and config step needs dropdown to overflow */
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

  /* Config step */
  .config-step {
    padding: 16px;
    padding-bottom: 24px; /* Extra padding to accommodate dropdown overflow */
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow: visible;
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 6px;
    font-size: var(--size-sm);
  }

  :global(.repo-info-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .repo-info-label {
    color: var(--text-muted);
  }

  .repo-info-value {
    color: var(--text-primary);
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-group label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .optional-label {
    font-weight: 400;
    color: var(--text-faint);
  }

  .subpath-input-container {
    position: relative;
  }

  .form-group input {
    width: 100%;
    padding: 10px 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: var(--size-md);
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s;
    box-sizing: border-box;
  }

  .form-group input:focus {
    border-color: var(--ui-accent);
  }

  .form-group input:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .form-group input::placeholder {
    color: var(--text-faint);
  }

  /* Subpath suggestions dropdown */
  .subpath-suggestions {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    z-index: 10;
    margin-top: 4px;
    max-height: 160px;
    overflow-y: auto;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    padding: 4px;
    background-color: var(--bg-primary);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
  }

  .subpath-suggestion {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    text-align: left;
    cursor: pointer;
    font-size: var(--size-sm);
    color: var(--text-primary);
    transition: background-color 0.1s;
  }

  .subpath-suggestion:hover,
  .subpath-suggestion.selected {
    background-color: var(--bg-hover);
  }

  :global(.suggestion-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .suggestion-name {
    font-family: 'SF Mono', 'Menlo', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .help-text {
    font-size: var(--size-xs);
    color: var(--text-muted);
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

  .cancel-button:hover:not(:disabled) {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .cancel-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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
