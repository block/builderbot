<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { AlertCircle } from 'lucide-svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import EmptyState from './lib/EmptyState.svelte';
  import TopBar from './lib/TopBar.svelte';
  import FileSearchModal from './lib/FileSearchModal.svelte';
  import TabBar from './lib/TabBar.svelte';
  import { listRefs } from './lib/services/git';
  import { getWindowLabel } from './lib/services/window';
  import {
    windowState,
    addTab,
    closeTab,
    switchTab,
    setWindowLabel,
    loadTabsFromStorage,
    getActiveTab,
  } from './lib/stores/tabState.svelte';
  import { createDiffState } from './lib/stores/diffState.svelte';
  import { createCommentsState } from './lib/stores/comments.svelte';
  import { createDiffSelection } from './lib/stores/diffSelection.svelte';
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
    commentsState,
    loadComments,
    setCurrentPath,
    clearComments,
    setReferenceFilesLoader,
  } from './lib/stores/comments.svelte';
  import {
    repoState,
    initRepoState,
    setCurrentRepo,
    openRepoPicker,
  } from './lib/stores/repoState.svelte';

  // UI State
  let unsubscribeWatcher: Unsubscribe | null = null;
  let showFileSearch = $state(false);
  let unsubscribeMenuOpenFolder: Unsubscribe | null = null;
  let unsubscribeMenuCloseTab: Unsubscribe | null = null;
  let unsubscribeMenuCloseWindow: Unsubscribe | null = null;

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

    // Save updated state back to tab
    syncGlobalToTab();
  }

  // Preset selection
  async function handlePresetSelect(preset: DiffPreset) {
    resetState();
    clearReferenceFiles();
    selectPreset(preset);
    await loadAll();

    // Save updated state back to tab
    syncGlobalToTab();
  }

  // Custom diff selection (from DiffSelectorModal or PRSelectorModal)
  async function handleCustomDiff(spec: DiffSpecType, label?: string, prNumber?: number) {
    resetState();
    clearReferenceFiles();
    selectCustomDiff(spec, label, prNumber);
    await loadAll();

    // Save updated state back to tab
    syncGlobalToTab();
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

      // Save updated state back to tab
      syncGlobalToTab();
    }
  }

  // Menu Event Handlers
  async function handleMenuOpenFolder() {
    // Add a new tab for the selected repo
    await handleNewTab();
  }

  function handleMenuCloseTab() {
    // Close the active tab
    const activeTab = getActiveTab();
    if (!activeTab) return;

    closeTab(activeTab.id);

    // Close window if no tabs left
    if (windowState.tabs.length === 0) {
      const window = getCurrentWindow();
      window.close();
      return;
    }

    // Sync the new active tab's state to global
    syncTabToGlobal();

    // Watch the new active tab's repo (fire-and-forget)
    const newTab = getActiveTab();
    if (newTab) {
      watchRepo(newTab.repoPath);
    }
  }

  async function handleMenuCloseWindow() {
    // Close the current window
    const window = getCurrentWindow();
    await window.close();
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

  /**
   * Extract repository name from path.
   */
  function extractRepoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
  }

  /**
   * Sync active tab's state to global singletons.
   * This allows existing components to work without changes.
   */
  function syncTabToGlobal() {
    const tab = getActiveTab();
    if (!tab) return;

    console.log(`Syncing tab "${tab.repoName}" to global state`);

    // Copy active tab's state to global singletons (property by property for reactivity)
    diffState.currentSpec = tab.diffState.currentSpec;
    diffState.currentRepoPath = tab.diffState.currentRepoPath;
    diffState.files = tab.diffState.files;
    diffState.diffCache = tab.diffState.diffCache;
    diffState.selectedFile = tab.diffState.selectedFile;
    diffState.scrollTargetLine = tab.diffState.scrollTargetLine;
    diffState.loading = tab.diffState.loading;
    diffState.loadingFile = tab.diffState.loadingFile;
    diffState.error = tab.diffState.error;

    commentsState.comments = tab.commentsState.comments;
    commentsState.reviewedPaths = tab.commentsState.reviewedPaths;
    commentsState.currentPath = tab.commentsState.currentPath;
    commentsState.currentSpec = tab.commentsState.currentSpec;
    commentsState.currentRepoPath = tab.commentsState.currentRepoPath;
    commentsState.loading = tab.commentsState.loading;

    diffSelection.spec = tab.diffSelection.spec;
    diffSelection.label = tab.diffSelection.label;
    diffSelection.prNumber = tab.diffSelection.prNumber;

    // Update repo state
    setCurrentRepo(tab.repoPath);
  }

  /**
   * Sync global singletons back to active tab.
   * Called after state changes to preserve tab state.
   */
  function syncGlobalToTab() {
    const tab = getActiveTab();
    if (!tab) return;

    console.log(`Saving global state to tab "${tab.repoName}"`);

    // Copy global state back to active tab
    tab.diffState.currentSpec = diffState.currentSpec;
    tab.diffState.currentRepoPath = diffState.currentRepoPath;
    tab.diffState.files = diffState.files;
    tab.diffState.diffCache = diffState.diffCache;
    tab.diffState.selectedFile = diffState.selectedFile;
    tab.diffState.scrollTargetLine = diffState.scrollTargetLine;
    tab.diffState.loading = diffState.loading;
    tab.diffState.loadingFile = diffState.loadingFile;
    tab.diffState.error = diffState.error;

    tab.commentsState.comments = commentsState.comments;
    tab.commentsState.reviewedPaths = commentsState.reviewedPaths;
    tab.commentsState.currentPath = commentsState.currentPath;
    tab.commentsState.currentSpec = commentsState.currentSpec;
    tab.commentsState.currentRepoPath = commentsState.currentRepoPath;
    tab.commentsState.loading = commentsState.loading;

    tab.diffSelection.spec = diffSelection.spec;
    tab.diffSelection.label = diffSelection.label;
    tab.diffSelection.prNumber = diffSelection.prNumber;
  }

  /**
   * Initialize a newly created tab with data.
   */
  async function initializeNewTab(tab: any) {
    try {
      // Load refs and detect default branch
      const refs = await listRefs(tab.repoPath);
      const defaultBranch = detectDefaultBranch(refs);
      setDefaultBranch(defaultBranch);

      // Reset to uncommitted preset
      resetDiffSelection();

      // Load files and comments
      await loadFiles(diffSelection.spec, tab.repoPath);
      await loadComments(diffSelection.spec, tab.repoPath);

      // Save state back to tab
      syncGlobalToTab();
    } catch (e) {
      console.error('Failed to initialize tab:', e);
      diffState.error = e instanceof Error ? e.message : String(e);
      diffState.loading = false;
    }
  }

  /**
   * Handle tab switching.
   */
  async function handleTabSwitch(index: number) {
    console.log(`Switching to tab ${index}`);

    // Save current tab state before switching
    syncGlobalToTab();

    // Switch to new tab
    await switchTab(index);
    console.log(`Active tab after switch:`, getActiveTab()?.repoName);

    // Load new tab state
    syncTabToGlobal();

    // Watch the new tab's repo and initialize if needed
    const tab = getActiveTab();
    if (tab) {
      console.log(`Watching repo: ${tab.repoPath}`);
      await watchRepo(tab.repoPath);

      // Initialize tab if it hasn't been loaded yet (e.g., restored from storage)
      if (tab.diffState.currentSpec === null) {
        await initializeNewTab(tab);
      }
    }
  }

  /**
   * Handle new tab creation.
   */
  async function handleNewTab() {
    const repoPath = await openRepoPicker();
    if (!repoPath) return;

    // Save current tab state before creating new one
    syncGlobalToTab();

    const repoName = extractRepoName(repoPath);
    addTab(repoPath, repoName, createDiffState, createCommentsState, createDiffSelection);

    // Sync to the new tab
    syncTabToGlobal();

    // Initialize the new tab
    const newTab = getActiveTab();
    if (newTab) {
      await initializeNewTab(newTab);
    }
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

      // Get window label and initialize tab state
      const label = await getWindowLabel();
      setWindowLabel(label);

      // Load tabs from storage (if any)
      loadTabsFromStorage(createDiffState, createCommentsState, createDiffSelection);

      // Initialize watcher listener once (handles all repos)
      unsubscribeWatcher = await initWatcher(handleFilesChanged);

      // Register menu event listeners
      unsubscribeMenuOpenFolder = await listen('menu:open-folder', handleMenuOpenFolder);
      unsubscribeMenuCloseTab = await listen('menu:close-tab', handleMenuCloseTab);
      unsubscribeMenuCloseWindow = await listen('menu:close-window', handleMenuCloseWindow);

      // Initialize repo state (resolves canonical path, adds to recent repos)
      const repoPath = await initRepoState();

      if (repoPath) {
        // Create initial tab if no tabs loaded from storage
        if (windowState.tabs.length === 0) {
          const repoName = extractRepoName(repoPath);
          addTab(repoPath, repoName, createDiffState, createCommentsState, createDiffSelection);
        }

        // Sync the active tab to global state
        syncTabToGlobal();

        // Watch the active tab's repo
        const tab = getActiveTab();
        if (tab) {
          await watchRepo(tab.repoPath);

          // Initialize the active tab
          await initializeNewTab(tab);
        }
      }
    })();
  });

  onDestroy(() => {
    unregisterPreferenceShortcuts?.();
    unregisterFileSearchShortcut?.();
    unsubscribeWatcher?.();
    unsubscribeMenuOpenFolder?.();
    unsubscribeMenuCloseTab?.();
    unsubscribeMenuCloseWindow?.();
  });
</script>

<main>
  {#if windowState.tabs.length > 0}
    <TabBar onNewTab={handleNewTab} onSwitchTab={handleTabSwitch} />
  {/if}

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
    background-color: var(--bg-primary);
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
