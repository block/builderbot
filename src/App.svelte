<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { AlertCircle } from 'lucide-svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import EmptyState from './lib/EmptyState.svelte';
  import TopBar from './lib/TopBar.svelte';
  import FileSearchModal from './lib/FileSearchModal.svelte';
  import { listRefs } from './lib/services/git';
  import { DiffSpec, inferRefType } from './lib/types';
  import type { DiffSpec as DiffSpecType } from './lib/types';
  import { initWatcher, watchRepo, type Unsubscribe } from './lib/services/statusEvents';
  import { referenceFileAsDiff } from './lib/diffUtils';
  import {
    addReferenceFile,
    removeReferenceFile,
    loadReferenceFiles,
    clearReferenceFiles,
    getReferenceFile,
    getReferenceFilePaths,
  } from './lib/stores/referenceFiles.svelte';
  import {
    preferences,
    loadSavedSize,
    loadSavedSyntaxTheme,
    registerPreferenceShortcuts,
  } from './lib/stores/preferences.svelte';
  import { registerShortcut } from './lib/services/keyboard';
  import {
    diffSelection,
    selectPreset,
    selectCustomDiff,
    resetDiffSelection,
    setDefaultBranch,
    type DiffPreset,
  } from './lib/stores/diffSelection.svelte';
  import {
    diffState,
    getCurrentDiff,
    loadFiles,
    refreshFiles,
    selectFile,
    resetState,
  } from './lib/stores/diffState.svelte';
  import {
    loadComments,
    setCurrentPath,
    clearComments,
    setReferenceFilesLoader,
  } from './lib/stores/comments.svelte';
  import { repoState, initRepoState, setCurrentRepo } from './lib/stores/repoState.svelte';

  // UI State
  let unsubscribeWatcher: Unsubscribe | null = null;
  let showFileSearch = $state(false);

  // Load files and comments for current spec
  async function loadAll() {
    const repoPath = repoState.currentPath ?? undefined;
    await loadFiles(diffSelection.spec, repoPath);
    await loadComments(diffSelection.spec, repoPath);
  }

  // Update comments store when selected file changes
  $effect(() => {
    const diff = getCurrentDiff();
    const path = diff?.after?.path ?? diff?.before?.path ?? null;
    setCurrentPath(path);
  });

  async function handleFilesChanged() {
    // Only refresh if viewing working tree
    if (diffSelection.spec.head.type !== 'WorkingTree') return;

    await refreshFiles(diffSelection.spec, repoState.currentPath ?? undefined);
    // Reload comments - they may have changed after a commit
    await loadComments(diffSelection.spec);
  }

  // Preset selection
  async function handlePresetSelect(preset: DiffPreset) {
    resetState();
    clearReferenceFiles();
    selectPreset(preset);
    await loadAll();
  }

  // Custom diff selection (from DiffSelectorModal or PRSelectorModal)
  async function handleCustomDiff(spec: DiffSpecType, label?: string, prNumber?: number) {
    resetState();
    clearReferenceFiles();
    selectCustomDiff(spec, label, prNumber);
    await loadAll();
  }

  // Repo change - reload everything
  async function handleRepoChange() {
    resetState();
    clearComments();
    clearReferenceFiles();

    if (repoState.currentPath) {
      watchRepo(repoState.currentPath);

      // Load refs and detect default branch for new repo
      try {
        const refs = await listRefs(repoState.currentPath);
        const defaultBranch = detectDefaultBranch(refs);
        setDefaultBranch(defaultBranch);
        // Mark repo as valid since we got refs
        setCurrentRepo(repoState.currentPath);
      } catch (e) {
        // Repo doesn't exist or isn't a git repo - show friendly error
        const errorMsg = e instanceof Error ? e.message : String(e);
        if (errorMsg.includes('No such file or directory')) {
          diffState.error = `Repository not found: ${repoState.currentPath}`;
        } else if (errorMsg.includes('not a git repository')) {
          diffState.error = `Not a git repository: ${repoState.currentPath}`;
        } else {
          diffState.error = errorMsg;
        }
        diffState.loading = false;
        console.error('Failed to load refs:', e);
        return;
      }

      // Reset diff selection to "Uncommitted" and load
      resetDiffSelection();
      await loadAll();
    }
  }

  /**
   * Detect the default branch (main, master, etc.) from available refs.
   */
  function detectDefaultBranch(refs: string[]): string {
    // Filter to likely branch names (not remotes, not tags)
    const branchNames = refs.filter((r) => inferRefType(r) === 'branch');

    // Check common default branch names in order of preference
    const candidates = ['main', 'master', 'develop', 'trunk'];
    for (const name of candidates) {
      if (branchNames.includes(name)) {
        return name;
      }
    }

    // Fallback to first branch, or 'main' if no branches
    return branchNames[0] ?? 'main';
  }

  // Get current diff - check reference files first
  // Check if current selection is a reference file
  let isCurrentFileReference = $derived(
    diffState.selectedFile !== null && getReferenceFile(diffState.selectedFile) !== undefined
  );

  let currentDiff = $derived.by(() => {
    const selectedPath = diffState.selectedFile;
    if (!selectedPath) return getCurrentDiff();

    // Check if it's a reference file
    const refFile = getReferenceFile(selectedPath);
    if (refFile) {
      return referenceFileAsDiff(refFile.path, refFile.content);
    }

    // Otherwise, get the regular diff
    return getCurrentDiff();
  });

  // Handle file selection from file search modal
  async function handleReferenceFileSelect(path: string) {
    try {
      // Determine which ref to use for loading the file
      // Use the "head" ref of the current diff
      const headRef = diffSelection.spec.head;
      const refName = headRef.type === 'WorkingTree' ? 'HEAD' : headRef.value;
      await addReferenceFile(refName, path, diffSelection.spec, repoState.currentPath ?? undefined);
      showFileSearch = false;
      // Select the newly added file
      selectFile(path);
    } catch (e) {
      console.error('Failed to add reference file:', e);
      // Keep modal open so user sees the error
    }
  }

  // Handle removing a reference file
  function handleRemoveReferenceFile(path: string) {
    removeReferenceFile(path, diffSelection.spec, repoState.currentPath ?? undefined);
  }

  // Show empty state when we have a repo, finished loading, no error, but no files
  let showEmptyState = $derived(
    repoState.currentPath && !diffState.loading && !diffState.error && diffState.files.length === 0
  );

  let isWorkingTree = $derived(diffSelection.spec.head.type === 'WorkingTree');

  // Lifecycle
  let unregisterPreferenceShortcuts: (() => void) | null = null;
  let unregisterFileSearchShortcut: (() => void) | null = null;

  onMount(() => {
    loadSavedSize();
    unregisterPreferenceShortcuts = registerPreferenceShortcuts();

    // Register Cmd+O to open file search
    unregisterFileSearchShortcut = registerShortcut({
      id: 'open-file-search',
      keys: ['o'],
      modifiers: { meta: true },
      description: 'Open file search',
      category: 'files',
      handler: () => {
        if (repoState.currentPath && !diffState.error) {
          showFileSearch = true;
        }
      },
    });

    // Register the reference files loader so comments store can trigger it
    setReferenceFilesLoader(loadReferenceFiles);

    (async () => {
      await loadSavedSyntaxTheme();

      // Initialize watcher listener once (handles all repos)
      unsubscribeWatcher = await initWatcher(handleFilesChanged);

      // Initialize repo state (resolves canonical path, adds to recent repos)
      const repoPath = await initRepoState();

      if (repoPath) {
        watchRepo(repoPath);

        // Load refs for autocomplete and detect default branch
        try {
          const refs = await listRefs(repoPath);
          const defaultBranch = detectDefaultBranch(refs);
          setDefaultBranch(defaultBranch);

          await loadAll();
        } catch (e) {
          // Initial load failed - not a git repo or other error
          diffState.loading = false;
          console.error('Failed to load refs:', e);
        }
      }
    })();
  });

  onDestroy(() => {
    unregisterPreferenceShortcuts?.();
    unregisterFileSearchShortcut?.();
    unsubscribeWatcher?.();
  });
</script>

<main>
  <TopBar
    onPresetSelect={handlePresetSelect}
    onCustomDiff={handleCustomDiff}
    onRepoChange={handleRepoChange}
    onCommit={handleFilesChanged}
  />

  <div class="app-container">
    {#if showEmptyState}
      <!-- Full-width empty state -->
      <section class="main-content full-width">
        <EmptyState />
      </section>
    {:else}
      <section class="main-content">
        {#if diffState.loading}
          <div class="loading-state">
            <p>Loading...</p>
          </div>
        {:else if diffState.error}
          <div class="error-state">
            <AlertCircle size={18} />
            <p class="error-message">{diffState.error}</p>
          </div>
        {:else}
          <DiffViewer
            diff={currentDiff}
            sizeBase={preferences.sizeBase}
            syntaxThemeVersion={preferences.syntaxThemeVersion}
            loading={diffState.loadingFile !== null}
            isReferenceFile={isCurrentFileReference}
          />
        {/if}
      </section>
      <aside class="sidebar">
        <Sidebar
          files={diffState.files}
          loading={diffState.loading}
          onFileSelect={selectFile}
          selectedFile={diffState.selectedFile}
          {isWorkingTree}
          onAddReferenceFile={() => (showFileSearch = true)}
          onRemoveReferenceFile={handleRemoveReferenceFile}
        />
      </aside>
    {/if}
  </div>
</main>

{#if showFileSearch}
  {@const headRef = diffSelection.spec.head}
  <FileSearchModal
    refName={headRef.type === 'WorkingTree' ? 'HEAD' : headRef.value}
    repoPath={repoState.currentPath ?? undefined}
    existingPaths={[
      ...diffState.files
        .map((f) => f.after?.toString() ?? f.before?.toString() ?? '')
        .filter(Boolean),
      ...getReferenceFilePaths(),
    ]}
    onSelect={handleReferenceFileSelect}
    onClose={() => (showFileSearch = false)}
  />
{/if}

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
    background-color: var(--bg-chrome);
    color: var(--text-primary);
  }

  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
    background-color: var(--bg-chrome);
  }

  .app-container {
    display: flex;
    flex: 1;
    overflow: hidden;
    padding: 0 8px 8px 8px;
    gap: 8px;
  }

  .sidebar {
    width: 260px;
    min-width: 180px;
    background-color: transparent;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .main-content {
    flex: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-lg);
  }

  .error-state {
    display: flex;
    flex-direction: row;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 100%;
    color: var(--text-muted);
  }

  .error-message {
    font-size: var(--size-md);
    margin: 0;
  }
</style>
