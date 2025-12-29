<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ChevronDown } from 'lucide-svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import CommitPanel from './lib/CommitPanel.svelte';
  import DiffSelectorModal from './lib/DiffSelectorModal.svelte';
  import { getRefDiff, resolveRef } from './lib/services/git';
  import {
    subscribeToStatusEvents,
    startWatching,
    stopWatching,
    type Unsubscribe,
  } from './lib/services/statusEvents';
  import type { FileDiff, GitStatus, DiffSpec } from './lib/types';

  // UI scaling
  const SIZE_STEP = 1;
  const SIZE_MIN = 10;
  const SIZE_MAX = 24;
  const SIZE_DEFAULT = 13;
  const SIZE_STORAGE_KEY = 'staged-size-base';

  let sizeBase = $state(SIZE_DEFAULT);

  function loadSavedSize() {
    const saved = localStorage.getItem(SIZE_STORAGE_KEY);
    if (saved) {
      const parsed = parseInt(saved, 10);
      if (!isNaN(parsed) && parsed >= SIZE_MIN && parsed <= SIZE_MAX) {
        sizeBase = parsed;
      }
    }
    applySize();
  }

  function applySize() {
    document.documentElement.style.setProperty('--size-base', `${sizeBase}px`);
  }

  function increaseSize() {
    if (sizeBase < SIZE_MAX) {
      sizeBase += SIZE_STEP;
      applySize();
      localStorage.setItem(SIZE_STORAGE_KEY, String(sizeBase));
    }
  }

  function decreaseSize() {
    if (sizeBase > SIZE_MIN) {
      sizeBase -= SIZE_STEP;
      applySize();
      localStorage.setItem(SIZE_STORAGE_KEY, String(sizeBase));
    }
  }

  function resetSize() {
    sizeBase = SIZE_DEFAULT;
    applySize();
    localStorage.setItem(SIZE_STORAGE_KEY, String(sizeBase));
  }

  function handleKeydown(event: KeyboardEvent) {
    // Cmd/Ctrl + Shift + = (plus) to increase size
    // Cmd/Ctrl + Shift + - (minus) to decrease size
    // Cmd/Ctrl + Shift + 0 to reset size
    if ((event.metaKey || event.ctrlKey) && event.shiftKey) {
      if (event.key === '=' || event.key === '+') {
        event.preventDefault();
        increaseSize();
      } else if (event.key === '-' || event.key === '_') {
        event.preventDefault();
        decreaseSize();
      } else if (event.key === '0') {
        event.preventDefault();
        resetSize();
      }
    }
  }

  // ==========================================================================
  // Diff Selector
  // ==========================================================================

  // Available diff presets
  const DIFF_PRESETS: DiffSpec[] = [
    { base: 'HEAD', head: '@', label: 'Working Changes' },
    { base: 'main', head: '@', label: 'Against main' },
    { base: 'HEAD~1', head: 'HEAD', label: 'Last Commit' },
  ];

  // Current diff spec - default to working changes
  let currentDiffSpec = $state<DiffSpec>(DIFF_PRESETS[0]);
  let diffSelectorOpen = $state(false);
  let customDiffModalOpen = $state(false);

  // Resolved SHAs for tooltip display
  let resolvedBaseSha = $state<string | null>(null);
  let resolvedHeadSha = $state<string | null>(null);

  // Derived values for easy access
  let diffBase = $derived(currentDiffSpec.base);
  let diffHead = $derived(currentDiffSpec.head);

  // Is this a preset or custom diff?
  let isPreset = $derived(
    DIFF_PRESETS.some(
      (p) => p.base === diffBase && p.head === diffHead && p.label === currentDiffSpec.label
    )
  );

  // Display label: short name for presets, base..head for custom
  let displayLabel = $derived(isPreset ? currentDiffSpec.label : `${diffBase}..${diffHead}`);

  // Tooltip with full details
  let tooltipText = $derived(() => {
    const basePart = resolvedBaseSha ? `${diffBase} (${resolvedBaseSha})` : diffBase;
    const headPart = resolvedHeadSha ? `${diffHead} (${resolvedHeadSha})` : diffHead;
    return `${basePart} → ${headPart}`;
  });

  async function updateResolvedShas() {
    try {
      resolvedBaseSha = await resolveRef(diffBase);
      resolvedHeadSha = await resolveRef(diffHead);
    } catch {
      resolvedBaseSha = null;
      resolvedHeadSha = null;
    }
  }

  function selectDiffSpec(spec: DiffSpec) {
    currentDiffSpec = spec;
    diffSelectorOpen = false;
    // Clear current selection and reload
    selectedFile = null;
    currentDiff = null;
    // Update resolved SHAs for tooltip
    updateResolvedShas();
    // Trigger sidebar reload with new diff spec
    sidebarRef?.loadStatus();
  }

  function handleCustomDiffSelect(base: string, head: string, label: string) {
    selectDiffSpec({ base, head, label });
  }

  function openCustomDiffModal() {
    diffSelectorOpen = false;
    customDiffModalOpen = true;
  }

  function toggleDiffSelector() {
    diffSelectorOpen = !diffSelectorOpen;
  }

  function closeDiffSelector() {
    diffSelectorOpen = false;
  }

  let selectedFile: string | null = $state(null);
  let currentDiff: FileDiff | null = $state(null);
  let diffError: string | null = $state(null);
  let sidebarRef: Sidebar | null = $state(null);
  let commitPanelRef: CommitPanel | null = $state(null);

  // Guard against concurrent diff loads
  let loadingPath: string | null = null;

  // Watcher cleanup function
  let unsubscribe: Unsubscribe | null = null;

  // Current repo path (for watcher)
  let currentRepoPath: string | null = $state(null);

  /**
   * Check if a file path exists in the given status (any category).
   */
  function fileExistsInStatus(status: GitStatus, path: string): boolean {
    return (
      status.staged.some((f) => f.path === path) ||
      status.unstaged.some((f) => f.path === path) ||
      status.untracked.some((f) => f.path === path)
    );
  }

  /**
   * Handle status updates from the file watcher.
   * Only relevant when diffHead is "@" (working tree).
   */
  async function handleStatusUpdate(status: GitStatus) {
    // Forward to sidebar
    sidebarRef?.setStatus(status);

    // Refresh commit panel
    commitPanelRef?.refresh();

    // Only reload diff if we're viewing the working tree
    if (diffHead !== '@') {
      return;
    }

    // Check if currently selected file still exists
    if (selectedFile) {
      if (!fileExistsInStatus(status, selectedFile)) {
        // File no longer has changes - clear the diff
        currentDiff = null;
        selectedFile = null;
      } else {
        // File still has changes - reload diff (content may have changed)
        await loadDiff(selectedFile);
      }
    }
  }

  onMount(async () => {
    // Load saved UI size preference
    loadSavedSize();

    // Listen for keyboard shortcuts
    window.addEventListener('keydown', handleKeydown);

    // Resolve initial SHAs for tooltip
    updateResolvedShas();

    // Subscribe to status events from the backend
    unsubscribe = await subscribeToStatusEvents(
      // On status update - handle refresh logic
      handleStatusUpdate,
      // On slow repo detected (optional one-time notification)
      () => {
        console.log(
          'Slow repository detected. Consider enabling git fsmonitor: git config core.fsmonitor true'
        );
        // Could show a toast/notification here in the future
      }
    );
  });

  onDestroy(() => {
    // Clean up keyboard listener
    window.removeEventListener('keydown', handleKeydown);

    // Clean up watcher and event listeners
    unsubscribe?.();
    stopWatching().catch(() => {
      // Ignore errors on cleanup
    });
  });

  // Called by Sidebar when it loads a repo
  async function handleRepoLoaded(repoPath: string) {
    if (repoPath && repoPath !== currentRepoPath) {
      currentRepoPath = repoPath;
      try {
        await startWatching(repoPath);
        console.log('Started watching:', repoPath);
      } catch (e) {
        console.error('Failed to start watcher:', e);
      }
    }
  }

  async function handleFileSelect(path: string) {
    selectedFile = path;
    await loadDiff(path);
  }

  async function loadDiff(path: string) {
    // Skip if already loading this exact path (prevents duplicate calls)
    if (loadingPath === path) {
      return;
    }

    loadingPath = path;
    diffError = null;

    try {
      const diff = await getRefDiff(diffBase, diffHead, path);

      // Only update if this is still the file we want
      if (loadingPath === path) {
        currentDiff = diff;
      }
    } catch (e) {
      if (loadingPath === path) {
        const errorMsg = e instanceof Error ? e.message : String(e);

        // "File not found" means the file no longer has changes
        // (e.g., all changes were discarded). Clear selection gracefully.
        if (errorMsg.includes('not found')) {
          currentDiff = null;
          selectedFile = null;
        } else {
          // Real error - show it
          diffError = errorMsg;
          currentDiff = null;
        }
      }
      console.error('Failed to load diff:', e);
    } finally {
      if (loadingPath === path) {
        loadingPath = null;
      }
    }
  }

  async function handleStatusChange() {
    // Sidebar staged/unstaged/discarded a file - refresh commit panel
    commitPanelRef?.refresh();

    // Reload diff if file still selected (content may have changed from discard)
    if (selectedFile) {
      await loadDiff(selectedFile);
    }
  }

  async function handleCommitComplete() {
    // Refresh sidebar and commit panel after successful commit
    await sidebarRef?.loadStatus();
    commitPanelRef?.refresh();
    // Clear the diff view since files may have changed
    currentDiff = null;
    selectedFile = null;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<main onclick={closeDiffSelector}>
  <!-- Diff selector header -->
  <header class="diff-header">
    <div class="diff-selector-container">
      <button
        class="diff-selector"
        class:open={diffSelectorOpen}
        onclick={(e) => {
          e.stopPropagation();
          toggleDiffSelector();
        }}
        title={tooltipText()}
      >
        <span class="diff-label">{displayLabel}</span>
        <ChevronDown size={14} />
      </button>

      {#if diffSelectorOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="diff-dropdown" onclick={(e) => e.stopPropagation()}>
          {#each DIFF_PRESETS as preset}
            <button
              class="diff-option"
              class:selected={preset.base === diffBase && preset.head === diffHead}
              onclick={() => selectDiffSpec(preset)}
            >
              <span class="option-label">{preset.label}</span>
              <span class="option-spec">{preset.base}..{preset.head}</span>
            </button>
          {/each}
          <div class="dropdown-divider"></div>
          <button class="diff-option" onclick={openCustomDiffModal}>
            <span class="option-label">Custom...</span>
          </button>
        </div>
      {/if}
    </div>
  </header>

  <div class="app-container">
    <section class="main-content">
      {#if diffError}
        <div class="error-state">
          <p>Error loading diff:</p>
          <p class="error-message">{diffError}</p>
        </div>
      {:else}
        <DiffViewer
          diff={currentDiff}
          filePath={selectedFile}
          {diffHead}
          {sizeBase}
          onRangeDiscard={handleStatusChange}
        />
      {/if}
    </section>
    <aside class="sidebar">
      <Sidebar
        bind:this={sidebarRef}
        onFileSelect={handleFileSelect}
        onStatusChange={handleStatusChange}
        onRepoLoaded={handleRepoLoaded}
        {selectedFile}
        {diffBase}
        {diffHead}
      />
    </aside>
  </div>
  {#if diffHead === '@'}
    <footer class="commit-panel">
      <CommitPanel bind:this={commitPanelRef} onCommitComplete={handleCommitComplete} />
    </footer>
  {/if}
</main>

<DiffSelectorModal
  open={customDiffModalOpen}
  onClose={() => (customDiffModalOpen = false)}
  onSelect={handleCustomDiffSelect}
  currentBase={diffBase}
  currentHead={diffHead}
/>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
    background-color: var(--bg-primary);
    color: var(--text-primary);
  }

  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  /* Diff selector header */
  .diff-header {
    display: flex;
    align-items: center;
    padding: 8px 16px;
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border-primary);
    flex-shrink: 0;
  }

  .diff-selector-container {
    position: relative;
  }

  .diff-selector {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
    background: var(--bg-tertiary);
    border: 1px solid var(--border-primary);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      background-color 0.15s,
      border-color 0.15s;
  }

  .diff-selector:hover {
    background-color: var(--bg-input);
    border-color: var(--text-muted);
  }

  .diff-selector.open {
    background-color: var(--bg-input);
    border-color: var(--text-link);
  }

  .diff-label {
    font-weight: 500;
  }

  .diff-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    margin-top: 4px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    min-width: 200px;
    z-index: 100;
    overflow: hidden;
  }

  .diff-option {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .diff-option:hover {
    background-color: var(--bg-tertiary);
  }

  .diff-option.selected {
    background-color: var(--ui-selection);
  }

  .option-label {
    font-weight: 500;
  }

  .option-spec {
    font-family: monospace;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .dropdown-divider {
    height: 1px;
    background: var(--border-primary);
    margin: 4px 0;
  }

  .app-container {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .sidebar {
    width: 280px;
    min-width: 200px;
    background-color: var(--bg-secondary);
    border-left: 1px solid var(--border-primary);
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

  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--status-deleted);
    font-size: var(--size-lg);
  }

  .error-message {
    font-family: monospace;
    font-size: var(--size-sm);
    color: var(--text-muted);
    margin-top: 8px;
  }

  .commit-panel {
    height: 120px;
    min-height: 80px;
    background-color: var(--bg-secondary);
    border-top: 1px solid var(--border-primary);
  }
</style>
