<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    ArrowRight,
    GitBranch,
    Tag,
    Diamond,
    AlertCircle,
    Copy,
    MessageSquare,
    Check,
    Trash2,
  } from 'lucide-svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import { getRepoInfo, getRefs, resolveRef } from './lib/services/git';
  import type { GitRef } from './lib/types';
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
    getPresets,
    WORKDIR,
    diffSelection,
    selectDiffSpec,
    selectCustomDiff,
    initDiffSelection,
    setDefaultBranch,
  } from './lib/stores/diffSelection.svelte';
  import type { DiffSpec } from './lib/types';
  import {
    diffState,
    getCurrentDiff,
    loadDiffs,
    refreshDiffs,
    selectFile,
    resetState,
  } from './lib/stores/diffState.svelte';
  import {
    commentsState,
    loadComments,
    setCurrentPath,
    copyCommentsToClipboard,
    deleteAllComments,
  } from './lib/stores/comments.svelte';

  // UI State
  let sidebarRef: Sidebar | null = $state(null);
  let unsubscribe: Unsubscribe | null = null;
  let copiedFeedback = $state(false);

  // Inline diff selector state
  let baseInput = $state('');
  let headInput = $state('');
  let selectorHovered = $state(false);
  let inputFocused = $state(false);
  let activeInput = $state<'base' | 'head' | null>(null);
  let allRefs = $state<GitRef[]>([]);
  let selectedSuggestionIndex = $state(0);

  // Error state for custom diff input
  let inputError = $state<string | null>(null);

  // Sync inputs with current selection
  $effect(() => {
    baseInput = diffSelection.spec.base;
    headInput = diffSelection.spec.head;
  });

  // Clear error when inputs change
  $effect(() => {
    const _ = [baseInput, headInput];
    inputError = null;
  });

  // Filtered refs for autocomplete
  // Filter out WORKDIR from base suggestions (it can only be used as head)
  let filteredRefs = $derived(
    activeInput
      ? allRefs.filter((r) => {
          // WORKDIR can only be used as head, not base
          if (activeInput === 'base' && r.name === WORKDIR) return false;
          const query = (activeInput === 'base' ? baseInput : headInput).toLowerCase();
          return r.name.toLowerCase().includes(query);
        })
      : []
  );

  // Get ref type for displaying icons in the input bar
  function getRefType(refName: string): 'branch' | 'tag' | 'special' | null {
    const ref = allRefs.find((r) => r.name === refName);
    if (ref) return ref.ref_type;
    // Special cases not in refs list
    if (refName === 'HEAD' || refName.startsWith('HEAD~') || refName.startsWith('HEAD^')) {
      return 'special';
    }
    return null;
  }

  // Show dropdown when hovering OR when an input is focused
  let showDropdown = $derived(selectorHovered || inputFocused);

  // Diff Loading
  async function loadAllDiffs() {
    await loadDiffs(diffSelection.spec.base, diffSelection.spec.head);
    await loadComments(diffSelection.spec.base, diffSelection.spec.head);
    sidebarRef?.setDiffs(diffState.diffs);
  }

  // Update comments store when selected file changes
  $effect(() => {
    const path = currentDiff?.after?.path ?? currentDiff?.before?.path ?? null;
    setCurrentPath(path);
  });

  async function handleFilesChanged() {
    if (diffSelection.spec.head !== WORKDIR) return;
    // Use refreshDiffs to avoid loading flicker - keeps content visible during fetch
    await refreshDiffs(diffSelection.spec.base, diffSelection.spec.head);
    // Reload comments - they may have changed after a commit
    await loadComments(diffSelection.spec.base, diffSelection.spec.head);
    sidebarRef?.setDiffs(diffState.diffs);
  }

  // Preset selection
  async function handlePresetSelect(spec: DiffSpec) {
    inputError = null;
    resetState();
    await selectDiffSpec(spec);
    await loadAllDiffs();
  }

  // Custom input submission
  async function submitCustomDiff() {
    inputError = null;

    // Validate: WORKDIR can only be used as head
    if (baseInput === WORKDIR) {
      inputError = 'WORKDIR can only be used as head (target), not base';
      return;
    }

    // Validate refs exist
    try {
      const baseSha = await resolveRef(baseInput);
      const headSha = await resolveRef(headInput);
      if (!baseSha) {
        inputError = `Cannot resolve base ref: ${baseInput}`;
        return;
      }
      if (!headSha) {
        inputError = `Cannot resolve head ref: ${headInput}`;
        return;
      }

      resetState();
      await selectCustomDiff(baseInput, headInput);
      await loadAllDiffs();
    } catch (e) {
      inputError = e instanceof Error ? e.message : String(e);
    }
  }

  function handleInputKeydown(event: KeyboardEvent, field: 'base' | 'head') {
    if (event.key === 'Escape') {
      activeInput = null;
      (event.target as HTMLInputElement).blur();
    } else if (event.key === 'Enter') {
      if (activeInput && filteredRefs.length > 0 && selectedSuggestionIndex < filteredRefs.length) {
        selectSuggestion(filteredRefs[selectedSuggestionIndex]);
        event.preventDefault();
      } else {
        submitCustomDiff();
        (event.target as HTMLInputElement).blur();
      }
    } else if (event.key === 'ArrowDown' && activeInput) {
      event.preventDefault();
      selectedSuggestionIndex = Math.min(selectedSuggestionIndex + 1, filteredRefs.length - 1);
    } else if (event.key === 'ArrowUp' && activeInput) {
      event.preventDefault();
      selectedSuggestionIndex = Math.max(selectedSuggestionIndex - 1, 0);
    } else if (event.key === 'Tab' && activeInput && filteredRefs.length > 0) {
      event.preventDefault();
      selectSuggestion(filteredRefs[selectedSuggestionIndex]);
    }
  }

  function selectSuggestion(ref: GitRef) {
    if (activeInput === 'base') {
      baseInput = ref.name;
    } else if (activeInput === 'head') {
      headInput = ref.name;
    }
    selectedSuggestionIndex = 0;
  }

  function handleFocus(field: 'base' | 'head') {
    activeInput = field;
    inputFocused = true;
    selectedSuggestionIndex = 0;
  }

  function handleBlur() {
    setTimeout(() => {
      activeInput = null;
      inputFocused = false;
      // If inputs differ from current selection, submit
      if (baseInput !== diffSelection.spec.base || headInput !== diffSelection.spec.head) {
        submitCustomDiff();
      }
    }, 150);
  }

  // Check if current selection matches a preset
  function isPresetSelected(preset: DiffSpec): boolean {
    return preset.base === diffSelection.spec.base && preset.head === diffSelection.spec.head;
  }

  // Handle copy comments with visual feedback
  async function handleCopyComments() {
    const success = await copyCommentsToClipboard();
    if (success) {
      copiedFeedback = true;
      setTimeout(() => {
        copiedFeedback = false;
      }, 1500);
    }
  }

  let currentDiff = $derived(getCurrentDiff());

  /**
   * Detect the default branch (main, master, etc.) from available refs.
   */
  function detectDefaultBranch(refs: GitRef[]): string {
    const branchNames = refs.filter((r) => r.ref_type === 'branch').map((r) => r.name);

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

  // Lifecycle
  onMount(() => {
    loadSavedSize();
    window.addEventListener('keydown', handlePreferenceKeydown);

    (async () => {
      await loadSavedSyntaxTheme();

      // Load refs for autocomplete and detect default branch
      try {
        allRefs = await getRefs();
        const defaultBranch = detectDefaultBranch(allRefs);
        setDefaultBranch(defaultBranch);
      } catch (e) {
        console.error('Failed to load refs:', e);
      }

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
  <header class="diff-header">
    <!-- Inline diff selector -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="diff-selector"
      onmouseenter={() => (selectorHovered = true)}
      onmouseleave={() => (selectorHovered = false)}
    >
      <div class="diff-inputs">
        <div class="ref-input-wrapper">
          <span class="input-icon">
            {#if getRefType(baseInput) === 'branch'}
              <GitBranch size={12} />
            {:else if getRefType(baseInput) === 'tag'}
              <Tag size={12} />
            {:else}
              <Diamond size={12} />
            {/if}
          </span>
          <input
            type="text"
            class="ref-input"
            bind:value={baseInput}
            placeholder="base"
            onfocus={() => handleFocus('base')}
            onblur={handleBlur}
            onkeydown={(e) => handleInputKeydown(e, 'base')}
            autocomplete="off"
            spellcheck="false"
          />
        </div>
        <span class="separator">
          <ArrowRight size={14} />
        </span>
        <div class="ref-input-wrapper">
          <span class="input-icon">
            {#if getRefType(headInput) === 'branch'}
              <GitBranch size={12} />
            {:else if getRefType(headInput) === 'tag'}
              <Tag size={12} />
            {:else}
              <Diamond size={12} />
            {/if}
          </span>
          <input
            type="text"
            class="ref-input"
            bind:value={headInput}
            placeholder="head"
            onfocus={() => handleFocus('head')}
            onblur={handleBlur}
            onkeydown={(e) => handleInputKeydown(e, 'head')}
            autocomplete="off"
            spellcheck="false"
          />
        </div>
      </div>

      {#if showDropdown}
        <div class="presets-dropdown">
          {#if activeInput && filteredRefs.length > 0}
            <!-- Autocomplete suggestions -->
            {#each filteredRefs.slice(0, 5) as ref, i}
              <button
                class="preset-option"
                class:selected={i === selectedSuggestionIndex}
                onmousedown={() => selectSuggestion(ref)}
              >
                <span class="option-icon">
                  {#if ref.ref_type === 'branch'}
                    <GitBranch size={12} />
                  {:else if ref.ref_type === 'tag'}
                    <Tag size={12} />
                  {:else}
                    <Diamond size={12} />
                  {/if}
                </span>
                <span class="option-name">{ref.name}</span>
              </button>
            {/each}
          {:else}
            <!-- Preset options -->
            {#each getPresets() as preset}
              <button
                class="preset-option"
                class:active={isPresetSelected(preset)}
                onmousedown={() => handlePresetSelect(preset)}
              >
                <span class="option-name">{preset.label}</span>
                <span class="option-spec">{preset.base}..{preset.head}</span>
              </button>
            {/each}
          {/if}
        </div>
      {/if}
    </div>

    <!-- Input error display -->
    {#if inputError}
      <div class="input-error">
        <AlertCircle size={14} />
        <span>{inputError}</span>
      </div>
    {/if}

    <!-- Comments section -->
    {#if commentsState.comments.length > 0}
      <div class="comments-section">
        <MessageSquare size={14} />
        <span class="comment-count">{commentsState.comments.length}</span>
        <button
          class="comments-action-btn"
          class:copied={copiedFeedback}
          onclick={handleCopyComments}
          title="Copy all comments"
        >
          {#if copiedFeedback}
            <Check size={12} />
          {:else}
            <Copy size={12} />
          {/if}
        </button>
        <button
          class="comments-action-btn delete-btn"
          onclick={deleteAllComments}
          title="Delete all comments"
        >
          <Trash2 size={12} />
        </button>
      </div>
    {/if}

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
          diff={currentDiff}
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

  /* Inline diff selector */
  .diff-selector {
    position: relative;
    display: flex;
    flex-direction: column;
    padding-bottom: 8px; /* Extra padding for hover area to reach dropdown */
    margin-bottom: -8px;
  }

  .diff-inputs {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .ref-input-wrapper {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    background: var(--bg-primary);
    border-radius: 6px;
    transition: background-color 0.1s;
  }

  .ref-input-wrapper:focus-within {
    background: var(--bg-hover);
  }

  .input-icon {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .ref-input {
    width: 160px;
    padding: 0;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    /* Truncate from left (show end of branch name) */
    direction: rtl;
    text-align: left;
    text-overflow: ellipsis;
    overflow: hidden;
    white-space: nowrap;
  }

  .ref-input::placeholder {
    color: var(--text-faint);
    direction: ltr;
  }

  .ref-input:focus {
    outline: none;
    /* Reset direction when editing */
    direction: ltr;
  }

  .separator {
    color: var(--text-faint);
    display: flex;
    align-items: center;
    user-select: none;
    flex-shrink: 0;
  }

  .presets-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    margin-top: -4px; /* Overlap with padding for seamless hover */
    min-width: 100%;
    background: var(--bg-elevated);
    border-radius: 6px;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
    z-index: 100;
  }

  .preset-option {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
    white-space: nowrap;
  }

  .preset-option:hover {
    background-color: var(--bg-hover);
  }

  .preset-option.selected,
  .preset-option.active {
    background-color: var(--bg-chrome);
  }

  .option-icon {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .option-name {
    flex: 1;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .option-spec {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    color: var(--text-faint);
    font-size: var(--size-xs);
    flex-shrink: 0;
    margin-left: auto;
  }

  /* Input error message */
  .input-error {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 12px;
    padding: 4px 8px;
    background-color: color-mix(in srgb, var(--status-deleted) 15%, transparent);
    border-radius: 4px;
    color: var(--status-deleted);
    font-size: var(--size-xs);
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

  /* Comments section */
  .comments-section {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    padding: 4px 8px;
    background-color: var(--bg-primary);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .comment-count {
    font-weight: 500;
    min-width: 1ch;
  }

  .comments-action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .comments-action-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .comments-action-btn.copied {
    color: var(--status-added);
  }

  .comments-action-btn.delete-btn:hover {
    color: var(--status-deleted);
  }

  /* Theme picker - minimal */
  .theme-picker {
    margin-left: 12px;
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
