<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ChevronDown } from 'lucide-svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import DiffSelectorModal from './lib/DiffSelectorModal.svelte';
  import { getRepoInfo } from './lib/services/git';
  import {
    subscribeToFileChanges,
    startWatching,
    stopWatching,
    type Unsubscribe,
  } from './lib/services/statusEvents';
  import {
    preferences,
    loadSavedSize,
    loadSavedSyntaxTheme,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    handlePreferenceKeydown,
  } from './lib/stores/preferences.svelte';
  import {
    DIFF_PRESETS,
    diffSelection,
    getDisplayLabel,
    getTooltipText,
    selectDiffSpec,
    selectCustomDiff,
    initDiffSelection,
  } from './lib/stores/diffSelection.svelte';
  import {
    diffState,
    getCurrentDiff,
    loadDiffs,
    selectFile,
    resetState,
  } from './lib/stores/diffState.svelte';

  // UI State
  let diffSelectorOpen = $state(false);
  let customDiffModalOpen = $state(false);
  let sidebarRef: Sidebar | null = $state(null);
  let unsubscribe: Unsubscribe | null = null;

  // Diff Loading
  async function loadAllDiffs() {
    await loadDiffs(diffSelection.spec.base, diffSelection.spec.head);
    sidebarRef?.setDiffs(diffState.diffs);
  }

  async function handleFilesChanged() {
    if (diffSelection.spec.head !== '@') return;
    await loadAllDiffs();
  }

  // Diff Selector
  async function handleDiffSelect(spec: (typeof DIFF_PRESETS)[number]) {
    diffSelectorOpen = false;
    resetState();
    await selectDiffSpec(spec);
    await loadAllDiffs();
  }

  async function handleCustomDiffSelect(base: string, head: string, label: string) {
    resetState();
    await selectCustomDiff(base, head, label);
    await loadAllDiffs();
  }

  function toggleDiffSelector() {
    diffSelectorOpen = !diffSelectorOpen;
  }

  // Close dropdown when clicking outside
  $effect(() => {
    if (!diffSelectorOpen) return;

    function handleClickOutside(e: MouseEvent) {
      const target = e.target as HTMLElement;
      if (!target.closest('.diff-selector-container')) {
        diffSelectorOpen = false;
      }
    }

    const timeoutId = setTimeout(() => {
      document.addEventListener('click', handleClickOutside);
    }, 0);

    return () => {
      clearTimeout(timeoutId);
      document.removeEventListener('click', handleClickOutside);
    };
  });

  // Lifecycle
  onMount(() => {
    loadSavedSize();
    window.addEventListener('keydown', handlePreferenceKeydown);

    (async () => {
      await loadSavedSyntaxTheme();
      await initDiffSelection();
      await loadAllDiffs();

      try {
        const info = await getRepoInfo();
        if (info?.repo_path) {
          await startWatching(info.repo_path);
        }
      } catch (e) {
        console.error('Failed to start watcher:', e);
      }

      unsubscribe = await subscribeToFileChanges(handleFilesChanged);
    })();
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handlePreferenceKeydown);
    unsubscribe?.();
    stopWatching().catch(() => {});
  });
</script>

<main>
  <!-- Diff selector header -->
  <header class="diff-header">
    <div class="diff-selector-container">
      <button
        class="diff-selector"
        class:open={diffSelectorOpen}
        onclick={toggleDiffSelector}
        title={getTooltipText()}
      >
        <span class="diff-label">{getDisplayLabel()}</span>
        <ChevronDown size={14} />
      </button>

      {#if diffSelectorOpen}
        <div class="diff-dropdown">
          {#each DIFF_PRESETS as preset}
            <button
              class="diff-option"
              class:selected={preset.base === diffSelection.spec.base &&
                preset.head === diffSelection.spec.head}
              onclick={() => handleDiffSelect(preset)}
            >
              <span class="option-label">{preset.label}</span>
              <span class="option-spec">{preset.base}..{preset.head}</span>
            </button>
          {/each}
          <div class="dropdown-divider"></div>
          <button
            class="diff-option"
            onclick={() => {
              diffSelectorOpen = false;
              customDiffModalOpen = true;
            }}
          >
            <span class="option-label">Custom...</span>
          </button>
        </div>
      {/if}
    </div>

    <!-- Theme picker -->
    <div class="theme-picker">
      <span class="picker-label">Theme:</span>
      <select
        class="theme-select"
        onchange={(e) => selectSyntaxTheme((e.target as HTMLSelectElement).value as any)}
      >
        {#each getAvailableSyntaxThemes() as name}
          <option value={name} selected={name === preferences.syntaxTheme}>{name}</option>
        {/each}
      </select>
    </div>
  </header>

  <div class="app-container">
    <section class="main-content">
      {#if diffState.loading}
        <div class="loading-state">
          <p>Loading...</p>
        </div>
      {:else if diffState.error}
        <div class="error-state">
          <p>Error loading diff:</p>
          <p class="error-message">{diffState.error}</p>
        </div>
      {:else}
        <DiffViewer
          diff={getCurrentDiff()}
          diffBase={diffSelection.spec.base}
          diffHead={diffSelection.spec.head}
          sizeBase={preferences.sizeBase}
          syntaxThemeVersion={preferences.syntaxThemeVersion}
        />
      {/if}
    </section>
    <aside class="sidebar">
      <Sidebar
        bind:this={sidebarRef}
        onFileSelect={selectFile}
        selectedFile={diffState.selectedFile}
        diffBase={diffSelection.spec.base}
        diffHead={diffSelection.spec.head}
      />
    </aside>
  </div>
</main>

<DiffSelectorModal
  open={customDiffModalOpen}
  onClose={() => (customDiffModalOpen = false)}
  onSelect={handleCustomDiffSelect}
  currentBase={diffSelection.spec.base}
  currentHead={diffSelection.spec.head}
/>

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

  /* Header - part of unified chrome, no border */
  .diff-header {
    display: flex;
    align-items: center;
    padding: 6px 12px;
    background-color: transparent;
    flex-shrink: 0;
  }

  .diff-selector-container {
    position: relative;
  }

  .diff-selector {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      background-color 0.15s,
      border-color 0.15s;
  }

  .diff-selector:hover {
    background-color: var(--bg-hover);
  }

  .diff-selector.open {
    background-color: var(--bg-hover);
    border-color: var(--border-muted);
  }

  .diff-label {
    font-weight: 500;
  }

  .diff-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    margin-top: 4px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: var(--shadow-elevated);
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
    background-color: var(--bg-hover);
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
    background: var(--border-subtle);
    margin: 4px 0;
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

  /* Theme picker - minimal */
  .theme-picker {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .picker-label {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .theme-select {
    padding: 2px 4px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
  }

  .theme-select:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .theme-select:focus {
    outline: none;
    border-color: var(--border-muted);
  }
</style>
