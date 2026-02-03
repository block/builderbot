<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { AlertCircle } from 'lucide-svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import EmptyState from './lib/EmptyState.svelte';

  import FileSearchModal from './lib/FileSearchModal.svelte';
  import FolderPickerModal from './lib/FolderPickerModal.svelte';
  import AgentSetupModal from './lib/AgentSetupModal.svelte';
  import TabBar from './lib/TabBar.svelte';
  import { listRefs } from './lib/services/git';
  import { getWindowLabel, installCli } from './lib/services/window';
  import {
    windowState,
    addTab,
    closeTab,
    switchTab,
    setWindowLabel,
    loadTabsFromStorage,
    getActiveTab,
    markRepoNeedsRefresh,
    clearNeedsRefresh,
  } from './lib/stores/tabState.svelte';
  import { createDiffState } from './lib/stores/diffState.svelte';
  import { createCommentsState } from './lib/stores/comments.svelte';
  import { createDiffSelection } from './lib/stores/diffSelection.svelte';
  import { createAgentState, agentGlobalState, type Artifact } from './lib/stores/agent.svelte';
  import { discoverAcpProviders } from './lib/services/ai';
  import { DiffSpec, gitRefName } from './lib/types';
  import type { DiffSpec as DiffSpecType } from './lib/types';
  import { initWatcher, watchRepo, type Unsubscribe } from './lib/services/statusEvents';
  import { referenceFileAsDiff } from './lib/diffUtils';
  import {
    referenceFilesState,
    createReferenceFilesState,
    addReferenceFile,
    removeReferenceFile,
    loadReferenceFiles,
    clearReferenceFiles,
    getReferenceFile,
    getReferenceFilePaths,
  } from './lib/stores/referenceFiles.svelte';
  import { findRecentRepos, type RecentRepo } from './lib/services/files';
  import {
    preferences,
    loadSavedSize,
    loadSavedSyntaxTheme,
    loadSavedSidebarPosition,
    loadSavedSidebarWidth,
    loadSavedFeatures,
    setSidebarWidth,
    resetSidebarWidth,
    getCustomKeyboardBindings,
    registerPreferenceShortcuts,
    loadSavedAiAgent,
    hasAiAgentSelected,
  } from './lib/stores/preferences.svelte';
  import { loadCustomBindings } from './lib/services/keyboard';
  import { registerShortcut } from './lib/services/keyboard';
  import {
    diffSelection,
    selectPreset,
    selectCustomDiff,
    resetDiffSelection,
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
    openRepo,
    getRecentRepos,
  } from './lib/stores/repoState.svelte';
  import {
    clearResults as clearSmartDiffResults,
    loadAnalysisFromDb,
  } from './lib/stores/smartDiff.svelte';

  // UI State
  let unsubscribeWatcher: Unsubscribe | null = null;
  let showFileSearch = $state(false);
  let showFolderPicker = $state(false);
  let showAgentSetupModal = $state(false);
  let unsubscribeWindowFocus: Unsubscribe | null = null;
  let suggestedRepos = $state<RecentRepo[]>([]);
  let unsubscribeMenuOpenFolder: Unsubscribe | null = null;
  let unsubscribeMenuCloseTab: Unsubscribe | null = null;
  let unsubscribeMenuCloseWindow: Unsubscribe | null = null;
  let unsubscribeMenuInstallCli: Unsubscribe | null = null;

  // Sidebar resize state
  let isDraggingSidebar = $state(false);
  let dragStartX = $state(0);
  let dragStartWidth = $state(0);

  // Sidebar resize handlers
  function handleSidebarResizeStart(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();

    isDraggingSidebar = true;
    dragStartX = e.clientX;
    dragStartWidth = preferences.sidebarWidth;

    document.addEventListener('mousemove', handleSidebarResizeMove);
    document.addEventListener('mouseup', handleSidebarResizeEnd);
  }

  function handleSidebarResizeMove(e: MouseEvent) {
    if (!isDraggingSidebar) return;

    const delta =
      preferences.sidebarPosition === 'left' ? e.clientX - dragStartX : dragStartX - e.clientX;

    const newWidth = dragStartWidth + delta;
    setSidebarWidth(newWidth);
  }

  function handleSidebarResizeEnd() {
    isDraggingSidebar = false;
    document.removeEventListener('mousemove', handleSidebarResizeMove);
    document.removeEventListener('mouseup', handleSidebarResizeEnd);
  }

  function handleSidebarResizeDoubleClick() {
    resetSidebarWidth();
  }

  // Load files, comments, and AI analysis for current spec
  async function loadAll() {
    const repoPath = repoState.currentPath ?? undefined;
    await loadFiles(diffSelection.spec, repoPath);
    await loadComments(diffSelection.spec, repoPath);
    // Load any saved AI analysis from database
    await loadAnalysisFromDb(repoPath ?? null, diffSelection.spec);
  }

  // Update comments store when selected file changes
  $effect(() => {
    const diff = getCurrentDiff();
    const path = diff?.after?.path ?? diff?.before?.path ?? null;
    setCurrentPath(path);
  });

  async function handleFilesChanged(changedRepoPath: string) {
    const activeTab = getActiveTab();

    // If this is NOT the active tab's repo, mark those tabs as needing refresh
    if (!activeTab || activeTab.repoPath !== changedRepoPath) {
      markRepoNeedsRefresh(changedRepoPath);
      return;
    }

    // Only refresh if viewing working tree
    if (diffSelection.spec.head.type !== 'WorkingTree') {
      // Mark as needing refresh for when user switches back to working tree
      activeTab.needsRefresh = true;
      console.debug('[App] Files changed but not viewing working tree, marked for refresh');
      return;
    }

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
    clearSmartDiffResults();

    selectPreset(preset);

    await loadAll();

    // Clear needsRefresh since we just loaded fresh data
    const tab = getActiveTab();
    if (tab) clearNeedsRefresh(tab);

    // Save updated state back to tab
    syncGlobalToTab();
  }

  // Custom diff selection (from DiffSelectorModal or PRSelectorModal)
  async function handleCustomDiff(spec: DiffSpecType, label?: string, prNumber?: number) {
    resetState();
    clearReferenceFiles();
    clearSmartDiffResults();
    selectCustomDiff(spec, label, prNumber);
    await loadAll();

    // Clear needsRefresh since we just loaded fresh data
    const tab = getActiveTab();
    if (tab) clearNeedsRefresh(tab);

    // Save updated state back to tab
    syncGlobalToTab();
  }

  // Repo change - reload everything
  async function handleRepoChange() {
    resetState();
    clearComments();
    clearReferenceFiles();
    clearSmartDiffResults();

    if (repoState.currentPath) {
      watchRepo(repoState.currentPath);

      // Validate repo by loading refs
      try {
        await listRefs(repoState.currentPath);
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

  async function handleMenuInstallCli() {
    try {
      const path = await installCli();
      alert(
        `CLI installed successfully!\n\nYou can now run:\n  staged          # open current directory\n  staged /path    # open specific directory\n\nInstalled to: ${path}`
      );
    } catch (e) {
      const error = e as Error;
      alert(
        `Failed to install CLI:\n\n${error.message || error}\n\nYou may need to run manually:\n  sudo cp /path/to/staged/bin/staged /usr/local/bin/`
      );
    }
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
   * Note: Agent state is passed directly as a prop, not synced through global singletons.
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

    referenceFilesState.files = tab.referenceFilesState.files;
    referenceFilesState.loading = tab.referenceFilesState.loading;
    referenceFilesState.error = tab.referenceFilesState.error;

    // Update repo state
    setCurrentRepo(tab.repoPath);
  }

  /**
   * Sync global singletons back to active tab.
   * Called after state changes to preserve tab state.
   * Note: Agent state is passed directly as a prop, not synced through global singletons.
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

    tab.referenceFilesState.files = referenceFilesState.files;
    tab.referenceFilesState.loading = referenceFilesState.loading;
    tab.referenceFilesState.error = referenceFilesState.error;
  }

  /**
   * Initialize a newly created tab with data.
   */
  async function initializeNewTab(tab: any) {
    try {
      // Validate repo by loading refs
      await listRefs(tab.repoPath);

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
   * Watcher is already running for the repo - no restart needed.
   */
  async function handleTabSwitch(index: number) {
    console.log(`Switching to tab ${index}`);

    // Save current tab state before switching
    syncGlobalToTab();

    // Switch to new tab (synchronous - no watcher restart)
    switchTab(index);
    console.log(`Active tab after switch:`, getActiveTab()?.repoName);

    // Clear smart diff results (they're per-diff, not persisted per-tab)
    clearSmartDiffResults();

    // Load new tab state
    syncTabToGlobal();

    // Initialize tab if it hasn't been loaded yet (e.g., restored from storage)
    const tab = getActiveTab();
    if (tab && tab.diffState.currentSpec === null) {
      initializeNewTab(tab);
    } else if (tab?.needsRefresh && diffSelection.spec.head.type === 'WorkingTree') {
      // Tab was marked dirty while inactive - refresh now
      console.debug(`[App] Tab "${tab.repoName}" needs refresh, loading files`);
      clearNeedsRefresh(tab);
      await refreshFiles(diffSelection.spec, repoState.currentPath ?? undefined);
      await loadComments(diffSelection.spec);
      syncGlobalToTab();
    }
  }

  /**
   * Handle new tab creation - show the folder picker modal.
   */
  function handleNewTab() {
    showFolderPicker = true;
  }

  /**
   * Handle folder selection from the picker modal.
   */
  async function handleFolderSelect(repoPath: string) {
    showFolderPicker = false;

    // Update repo state
    openRepo(repoPath);

    // Save current tab state before creating new one
    syncGlobalToTab();

    const repoName = extractRepoName(repoPath);
    addTab(
      repoPath,
      repoName,
      createDiffState,
      createCommentsState,
      createDiffSelection,
      createAgentState,
      createReferenceFilesState
    );

    // Start watching the new repo (idempotent - won't restart if already watching)
    watchRepo(repoPath);

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
      const refName = gitRefName(diffSelection.spec.head);
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
    loadSavedSidebarPosition();
    loadSavedSidebarWidth();
    loadSavedFeatures();
    unregisterPreferenceShortcuts = registerPreferenceShortcuts();

    // Check if AI agent has been selected, show setup modal if not
    const hasAgent = loadSavedAiAgent();
    if (!hasAgent) {
      showAgentSetupModal = true;
    }

    // Pre-load suggested repos (Spotlight search runs in background)
    findRecentRepos(24, 10).then((repos) => {
      suggestedRepos = repos;
    });

    // Apply custom keyboard bindings after a short delay to let shortcuts register
    setTimeout(() => {
      loadCustomBindings(getCustomKeyboardBindings());
    }, 100);

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
      loadTabsFromStorage(
        createDiffState,
        createCommentsState,
        createDiffSelection,
        createAgentState,
        createReferenceFilesState
      );

      // Initialize watcher listener once (handles all repos)
      unsubscribeWatcher = await initWatcher(handleFilesChanged);

      // Start watchers for all restored tabs (idempotent - dedupes same repos)
      for (const tab of windowState.tabs) {
        watchRepo(tab.repoPath);
      }

      // Register menu event listeners
      unsubscribeMenuOpenFolder = await listen('menu:open-folder', handleMenuOpenFolder);
      unsubscribeMenuCloseTab = await listen('menu:close-tab', handleMenuCloseTab);
      unsubscribeMenuCloseWindow = await listen('menu:close-window', handleMenuCloseWindow);
      unsubscribeMenuInstallCli = await listen('menu:install-cli', handleMenuInstallCli);

      // Listen for window focus to refresh AI providers (user may have installed one)
      const currentWindow = getCurrentWindow();
      unsubscribeWindowFocus = await currentWindow.onFocusChanged(async ({ payload: focused }) => {
        if (focused) {
          try {
            const providers = await discoverAcpProviders();
            agentGlobalState.availableProviders = providers;
            agentGlobalState.providersLoaded = true;
          } catch (e) {
            console.error('Failed to refresh providers on focus:', e);
          }
        }
      });

      // Initialize repo state (resolves canonical path, adds to recent repos)
      const repoPath = await initRepoState();

      if (repoPath) {
        // Check if we already have a tab for this repo
        const existingTabIndex = windowState.tabs.findIndex((t) => t.repoPath === repoPath);

        if (existingTabIndex >= 0) {
          // Switch to existing tab for this repo
          switchTab(existingTabIndex);
        } else {
          // Create new tab for the CLI path
          const repoName = extractRepoName(repoPath);
          addTab(
            repoPath,
            repoName,
            createDiffState,
            createCommentsState,
            createDiffSelection,
            createAgentState,
            createReferenceFilesState
          );
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
      } else if (windowState.tabs.length > 0) {
        // No CLI path but we have restored tabs - use them
        syncTabToGlobal();

        const tab = getActiveTab();
        if (tab) {
          await watchRepo(tab.repoPath);
          // Initialize if needed
          if (tab.diffState.currentSpec === null) {
            await initializeNewTab(tab);
          }
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
    unsubscribeMenuInstallCli?.();
    unsubscribeWindowFocus?.();
    // Cleanup sidebar resize listeners
    document.removeEventListener('mousemove', handleSidebarResizeMove);
    document.removeEventListener('mouseup', handleSidebarResizeEnd);
  });
</script>

<main>
  {#if windowState.tabs.length > 0}
    <TabBar onNewTab={handleNewTab} onSwitchTab={handleTabSwitch} />
  {:else}
    <!-- Spacer for traffic light buttons when no tabs -->
    <div class="titlebar-spacer" data-tauri-drag-region></div>
  {/if}

  <div class="app-container" class:sidebar-left={preferences.sidebarPosition === 'left'}>
    <section class="main-content">
      {#if showEmptyState}
        <EmptyState />
      {:else if diffState.loading}
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
          agentState={getActiveTab()?.agentState}
        />
      {/if}
    </section>
    <aside class="sidebar" style="--sidebar-width: {preferences.sidebarWidth}">
      <!-- Resize handle -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="sidebar-resize-handle"
        class:left={preferences.sidebarPosition === 'left'}
        class:dragging={isDraggingSidebar}
        onmousedown={handleSidebarResizeStart}
        ondblclick={handleSidebarResizeDoubleClick}
      >
        <div class="resize-handle-bar"></div>
      </div>

      <Sidebar
        files={diffState.files}
        loading={diffState.loading}
        onFileSelect={selectFile}
        selectedFile={diffState.selectedFile}
        {isWorkingTree}
        onAddReferenceFile={() => (showFileSearch = true)}
        onRemoveReferenceFile={handleRemoveReferenceFile}
        repoPath={repoState.currentPath}
        spec={diffSelection.spec}
        agentState={getActiveTab()?.agentState}
        onPresetSelect={handlePresetSelect}
        onCustomDiff={handleCustomDiff}
        onReloadCommentsForTab={async (spec, repoPath) => {
          await loadComments(spec, repoPath ?? undefined);
          syncGlobalToTab();
        }}
      />
    </aside>
  </div>
</main>

{#if showFileSearch}
  <FileSearchModal
    refName={gitRefName(diffSelection.spec.head)}
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

{#if showFolderPicker}
  <FolderPickerModal
    recentRepos={getRecentRepos()}
    {suggestedRepos}
    currentPath={repoState.currentPath}
    onSelect={handleFolderSelect}
    onClose={() => (showFolderPicker = false)}
  />
{/if}

{#if showAgentSetupModal}
  <AgentSetupModal onComplete={() => (showAgentSetupModal = false)} />
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

  .titlebar-spacer {
    height: 28px;
    flex-shrink: 0;
    background: var(--bg-chrome);
    -webkit-app-region: drag;
  }

  .app-container {
    display: flex;
    flex: 1;
    overflow: hidden;
    padding: 12px 8px;
    gap: 8px;
  }

  .app-container.sidebar-left .main-content {
    order: 1;
  }

  .app-container.sidebar-left .sidebar {
    order: 0;
  }

  .sidebar {
    width: calc(var(--sidebar-width) * 1px);
    min-width: 180px;
    background-color: transparent;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
  }

  .sidebar-resize-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 8px;
    cursor: col-resize;
    z-index: 100;
    display: flex;
    align-items: stretch;
  }

  .sidebar-resize-handle.left {
    right: 0;
  }

  .sidebar-resize-handle:not(.left) {
    left: 0;
  }

  .resize-handle-bar {
    margin: auto;
    width: 4px;
    height: 100%;
    background-color: var(--border-muted);
    border-radius: 2px;
    opacity: 0;
    transition: opacity 0.15s ease;
    pointer-events: none;
  }

  .sidebar-resize-handle:hover .resize-handle-bar,
  .sidebar-resize-handle.dragging .resize-handle-bar {
    opacity: 1;
  }

  .sidebar-resize-handle.dragging .resize-handle-bar {
    background-color: var(--accent-primary);
  }

  .app-container:has(.sidebar-resize-handle.dragging) {
    user-select: none;
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
