<script lang="ts">
  import { onMount } from 'svelte';
  import {
    ChevronDown,
    Palette,
    Keyboard,
    MessageSquare,
    Copy,
    Check,
    Trash2,
    Settings2,
    GitCompareArrows,
    GitPullRequest,
    GitCommitHorizontal,
    Upload,
    Orbit,
    Eye,
    EyeOff,
  } from 'lucide-svelte';
  import DiffSelectorModal from './DiffSelectorModal.svelte';
  import PRSelectorModal from './PRSelectorModal.svelte';
  import CommitModal from './CommitModal.svelte';
  import ThemeSelectorModal from './ThemeSelectorModal.svelte';
  import GitHubSyncModal from './GitHubSyncModal.svelte';
  import KeyboardShortcutsModal from './KeyboardShortcutsModal.svelte';
  import SettingsModal from './SettingsModal.svelte';
  import SmartDiffModal from './SmartDiffModal.svelte';
  import { DiffSpec, gitRefDisplay } from './types';
  import type { DiffSpec as DiffSpecType } from './types';
  import {
    getPresets,
    diffSelection,
    getDisplayLabel,
    type DiffPreset,
  } from './stores/diffSelection.svelte';
  import { diffState } from './stores/diffState.svelte';
  import {
    commentsState,
    copyCommentsToClipboard,
    deleteAllComments,
  } from './stores/comments.svelte';
  import { repoState } from './stores/repoState.svelte';
  import { registerShortcut } from './services/keyboard';
  import {
    smartDiffState,
    checkAi,
    runAnalysis,
    deleteAnalysis,
    clearResults as clearSmartDiffState,
    setAnnotationsRevealed,
  } from './stores/smartDiff.svelte';

  interface Props {
    onPresetSelect: (preset: DiffPreset) => void;
    onCustomDiff: (spec: DiffSpecType, label?: string, prNumber?: number) => Promise<void>;
    onCommit?: () => void;
  }

  let { onPresetSelect, onCustomDiff, onCommit }: Props = $props();

  // Dropdown states
  let diffDropdownOpen = $state(false);

  // Modal state
  let showCustomModal = $state(false);
  let showPRModal = $state(false);
  let showCommitModal = $state(false);
  let showThemeModal = $state(false);
  let showSyncModal = $state(false);
  let showShortcutsModal = $state(false);
  let showSettingsModal = $state(false);
  let showSmartDiffModal = $state(false);

  // Copy feedback
  let copiedFeedback = $state(false);

  // Check if we're viewing working directory changes (can show commit button)
  let isWorkingTree = $derived(diffSelection.spec.head.type === 'WorkingTree');
  // Can only commit if there are files to commit
  let canCommit = $derived(isWorkingTree && diffState.files.length > 0);
  // Can sync if viewing a PR with comments
  let canSync = $derived(diffSelection.prNumber !== undefined && commentsState.comments.length > 0);

  // Smart diff state
  let isAiLoading = $derived(smartDiffState.loading);
  let hasAiResults = $derived(smartDiffState.changesetSummary !== null);
  let canRunAi = $derived(diffState.files.length > 0 && !diffState.loading);
  let annotationsRevealed = $derived(smartDiffState.annotationsRevealed);
  let hasFileAnnotations = $derived(smartDiffState.results.size > 0);

  // Check if current selection matches a preset
  function isPresetSelected(preset: DiffPreset): boolean {
    return DiffSpec.display(preset.spec) === DiffSpec.display(diffSelection.spec);
  }

  // Get current display label
  let currentLabel = $derived(getDisplayLabel());

  function handlePresetSelect(preset: DiffPreset) {
    diffDropdownOpen = false;
    onPresetSelect(preset);
  }

  function handleCustomClick() {
    diffDropdownOpen = false;
    showCustomModal = true;
  }

  function handlePRClick() {
    diffDropdownOpen = false;
    showPRModal = true;
  }

  async function handlePRSubmit(spec: DiffSpecType, label: string, prNumber: number) {
    // Modal will close itself after this completes
    await onCustomDiff(spec, label, prNumber);
  }

  function handleCustomSubmit(spec: DiffSpecType) {
    showCustomModal = false;
    onCustomDiff(spec);
  }

  async function handleCopyComments() {
    const success = await copyCommentsToClipboard();
    if (success) {
      copiedFeedback = true;
      setTimeout(() => {
        copiedFeedback = false;
      }, 1500);
    }
  }

  /**
   * Handle AI analysis button click.
   * If results exist, show modal. Otherwise, trigger analysis.
   */
  async function handleAiAnalysis() {
    // If we have results, just open the modal
    if (hasAiResults) {
      showSmartDiffModal = true;
      return;
    }

    // If already loading, do nothing (button shows progress)
    if (isAiLoading) return;

    if (!canRunAi) return;

    // Check AI availability first
    const available = await checkAi();
    if (!available) {
      // TODO: Show error toast or modal
      console.error('AI not available:', smartDiffState.aiError);
      return;
    }

    // Start analysis in background - don't open modal yet
    // The button will show loading state, modal opens when done
    runChangesetAnalysis();
  }

  /**
   * Run changeset analysis in background.
   * Opens modal automatically when complete.
   */
  async function runChangesetAnalysis() {
    try {
      // Single call - backend handles file listing and content loading
      const result = await runAnalysis(repoState.currentPath ?? null, diffSelection.spec);

      if (result) {
        // Analysis complete - open modal to show results
        showSmartDiffModal = true;
      }
    } catch (e) {
      console.error('Analysis failed:', e);
    }
  }

  /**
   * Handle refresh button click in the AI modal.
   * Deletes existing analysis and re-runs.
   */
  async function handleRefreshAnalysis() {
    // Clear existing results from memory and database
    await deleteAnalysis(repoState.currentPath ?? null, diffSelection.spec);
    clearSmartDiffState();

    // Re-run analysis
    await runChangesetAnalysis();
  }

  /**
   * Get display string for a DiffSpec in the dropdown
   */
  function getSpecDisplay(spec: DiffSpecType): string {
    return DiffSpec.display(spec);
  }

  /**
   * Get initial base string for the custom modal
   */
  function getInitialBase(): string {
    return gitRefDisplay(diffSelection.spec.base);
  }

  /**
   * Get initial head string for the custom modal
   */
  function getInitialHead(): string {
    return gitRefDisplay(diffSelection.spec.head);
  }

  // Close dropdowns when clicking outside
  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest('.diff-selector')) {
      diffDropdownOpen = false;
    }
  }

  // Register keyboard shortcuts
  onMount(() => {
    const unregisterCopy = registerShortcut({
      id: 'copy-comments',
      keys: ['c'],
      description: 'Copy all comments',
      category: 'comments',
      handler: () => {
        if (commentsState.comments.length > 0) {
          handleCopyComments();
        }
      },
    });

    const unregisterTheme = registerShortcut({
      id: 'open-theme-picker',
      keys: ['p'],
      modifiers: { meta: true },
      description: 'Theme picker',
      category: 'view',
      handler: () => {
        showThemeModal = !showThemeModal;
      },
    });

    return () => {
      unregisterCopy();
      unregisterTheme();
    };
  });
</script>

<svelte:window onclick={handleClickOutside} />

<header class="top-bar">
  <!-- Left section: Diff selector -->
  <div class="section section-left">
    <div class="diff-selector">
      <button
        class="diff-selector-btn"
        onclick={() => (diffDropdownOpen = !diffDropdownOpen)}
        class:open={diffDropdownOpen}
      >
        <GitCompareArrows size={14} />
        <span class="diff-label">{currentLabel}</span>
        <ChevronDown size={12} />
      </button>

      {#if diffDropdownOpen}
        <div class="dropdown diff-dropdown">
          {#each getPresets() as preset}
            <button
              class="dropdown-item diff-item"
              class:active={isPresetSelected(preset)}
              onclick={() => handlePresetSelect(preset)}
            >
              <GitCompareArrows size={14} />
              <div class="diff-item-content">
                <span class="diff-item-label">{preset.label}</span>
                <span class="diff-item-spec">{getSpecDisplay(preset.spec)}</span>
              </div>
            </button>
          {/each}
          <div class="dropdown-divider"></div>
          <button class="dropdown-item custom-item" onclick={handlePRClick}>
            <GitPullRequest size={12} />
            <span>Pull Request...</span>
          </button>
          <button class="dropdown-item custom-item" onclick={handleCustomClick}>
            <Settings2 size={12} />
            <span>Custom range...</span>
          </button>
        </div>
      {/if}
    </div>
  </div>

  <!-- Center section: Actions (Commit, Comments, AI) -->
  <div class="section section-center">
    {#if isWorkingTree}
      <button
        class="action-btn"
        class:disabled={!canCommit}
        onclick={() => canCommit && (showCommitModal = true)}
        title={canCommit ? 'Commit' : 'No staged or unstaged changes'}
        disabled={!canCommit}
      >
        <GitCommitHorizontal size={14} />
      </button>
    {/if}

    <!-- AI Analysis button -->
    <button
      class="action-btn ai-btn"
      class:loading={isAiLoading}
      class:has-results={hasAiResults}
      class:disabled={!canRunAi}
      onclick={handleAiAnalysis}
      title={isAiLoading ? 'Analyzing...' : hasAiResults ? 'View AI analysis' : 'Analyze with AI'}
      disabled={!canRunAi && !hasAiResults}
    >
      <div class="ai-icon" class:spinning={isAiLoading}>
        <Orbit size={14} />
      </div>
    </button>

    <!-- AI Annotations reveal toggle (only show when annotations exist) -->
    {#if hasFileAnnotations}
      <button
        class="action-btn reveal-btn"
        class:active={annotationsRevealed}
        onclick={() => setAnnotationsRevealed(!annotationsRevealed)}
        title="Hold A to show explanation view"
      >
        {#if annotationsRevealed}
          <Eye size={14} />
        {:else}
          <EyeOff size={14} />
        {/if}
      </button>
    {/if}

    <div class="comments-section">
      <MessageSquare size={14} />
      <span class="comment-count">{commentsState.comments.length}</span>
      {#if commentsState.comments.length > 0}
        {#if canSync}
          <button
            class="icon-btn sync-btn"
            onclick={() => (showSyncModal = true)}
            title="Sync comments to GitHub"
          >
            <Upload size={12} />
          </button>
        {/if}
        <button
          class="icon-btn"
          class:copied={copiedFeedback}
          onclick={handleCopyComments}
          title="Copy all comments (c)"
        >
          {#if copiedFeedback}
            <Check size={12} />
          {:else}
            <Copy size={12} />
          {/if}
        </button>
        <button class="icon-btn delete-btn" onclick={deleteAllComments} title="Delete all comments">
          <Trash2 size={12} />
        </button>
      {/if}
    </div>
  </div>

  <!-- Right section: Settings -->
  <div class="section section-right">
    <button
      class="icon-btn settings-btn"
      onclick={() => (showSettingsModal = true)}
      title="Settings"
    >
      <Settings2 size={14} />
    </button>

    <div class="shortcuts-picker">
      <button
        class="icon-btn shortcuts-btn"
        onclick={() => (showShortcutsModal = !showShortcutsModal)}
        class:open={showShortcutsModal}
        title="Keyboard shortcuts"
      >
        <Keyboard size={14} />
      </button>

      {#if showShortcutsModal}
        <KeyboardShortcutsModal onClose={() => (showShortcutsModal = false)} />
      {/if}
    </div>

    <div class="theme-picker">
      <button
        class="icon-btn theme-btn"
        onclick={() => (showThemeModal = !showThemeModal)}
        class:open={showThemeModal}
        title="Select theme"
      >
        <Palette size={14} />
      </button>

      {#if showThemeModal}
        <ThemeSelectorModal onClose={() => (showThemeModal = false)} />
      {/if}
    </div>
  </div>
</header>

{#if showCustomModal}
  <DiffSelectorModal
    initialBase={getInitialBase()}
    initialHead={getInitialHead()}
    onSubmit={handleCustomSubmit}
    onClose={() => (showCustomModal = false)}
  />
{/if}

{#if showPRModal}
  <PRSelectorModal
    repoPath={repoState.currentPath}
    onSubmit={handlePRSubmit}
    onClose={() => (showPRModal = false)}
  />
{/if}

{#if showCommitModal}
  <CommitModal
    repoPath={repoState.currentPath}
    onCommit={() => {
      showCommitModal = false;
      onCommit?.();
    }}
    onClose={() => (showCommitModal = false)}
  />
{/if}

{#if showSyncModal && diffSelection.prNumber}
  <GitHubSyncModal
    prNumber={diffSelection.prNumber}
    spec={diffSelection.spec}
    repoPath={repoState.currentPath}
    comments={commentsState.comments}
    onClose={() => (showSyncModal = false)}
  />
{/if}

{#if showSettingsModal}
  <SettingsModal onClose={() => (showSettingsModal = false)} />
{/if}

{#if showSmartDiffModal}
  <SmartDiffModal onClose={() => (showSmartDiffModal = false)} onRefresh={handleRefreshAnalysis} />
{/if}

<style>
  .top-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px;
    background-color: transparent;
    flex-shrink: 0;
    gap: 12px;
  }

  .section {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .section-left {
    flex: 1;
    justify-content: flex-start;
  }

  .section-center {
    flex: 0 0 auto;
  }

  .section-right {
    flex: 1;
    justify-content: flex-end;
  }

  /* Diff selector */
  .diff-selector {
    position: relative;
  }

  .diff-selector-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    background: var(--bg-primary);
    border: none;
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .diff-selector-btn:hover,
  .diff-selector-btn.open {
    background: var(--bg-hover);
  }

  .diff-selector-btn :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .diff-selector-btn :global(svg:last-child) {
    transition: transform 0.15s;
  }

  .diff-selector-btn.open :global(svg:last-child) {
    transform: rotate(180deg);
  }

  .diff-label {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Dropdowns */
  .dropdown {
    position: absolute;
    top: 100%;
    margin-top: 4px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
    z-index: 100;
    min-width: 100%;
  }

  .diff-dropdown {
    left: 0;
    width: 290px;
    padding-bottom: 4px;
  }

  .diff-item {
    align-items: flex-start;
  }

  .diff-item :global(svg) {
    flex-shrink: 0;
    margin-top: 2px;
  }

  .diff-item-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .diff-item-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .diff-item-spec {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dropdown-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-xs);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .dropdown-item:hover {
    background-color: var(--bg-hover);
  }

  .dropdown-item.active {
    background-color: var(--bg-primary);
  }

  .dropdown-divider {
    height: 1px;
    background: var(--border-subtle);
    margin: 4px 0;
  }

  .custom-item {
    color: var(--text-muted);
  }

  .custom-item :global(svg) {
    color: var(--text-muted);
  }

  /* Comments section */
  .comments-section {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    height: 24px;
    background-color: var(--bg-primary);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .comment-count {
    font-weight: 500;
    min-width: 1ch;
  }

  /* Icon buttons */
  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .icon-btn.copied {
    color: var(--status-added);
  }

  .icon-btn.delete-btn:hover {
    color: var(--status-deleted);
  }

  .icon-btn.sync-btn:hover {
    color: var(--ui-accent);
  }

  /* Shortcuts picker */
  .shortcuts-picker {
    position: relative;
  }

  .shortcuts-btn {
    padding: 5px;
    background: var(--bg-primary);
    border-radius: 6px;
  }

  .shortcuts-btn:hover,
  .shortcuts-btn.open {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Settings button */
  .settings-btn {
    padding: 5px;
    background: var(--bg-primary);
    border-radius: 6px;
  }

  .settings-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Theme picker */
  .theme-picker {
    position: relative;
  }

  .theme-btn {
    padding: 5px;
    background: var(--bg-primary);
    border-radius: 6px;
  }

  .theme-btn:hover,
  .theme-btn.open {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Action button (Commit, etc.) - icon only, label on hover */
  .action-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    height: 24px;
    background: var(--bg-primary);
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .action-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .action-btn:disabled,
  .action-btn.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-btn :global(svg) {
    flex-shrink: 0;
  }

  /* AI Analysis button */
  .ai-btn {
    position: relative;
    overflow: visible;
  }

  .ai-btn.has-results {
    color: var(--ui-accent);
  }

  .ai-btn.has-results:hover:not(:disabled) {
    color: var(--ui-accent);
  }

  .ai-icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .ai-icon.spinning {
    animation: spin 2s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  /* AI Annotations reveal toggle */
  .reveal-btn.active {
    color: var(--ui-accent);
  }

  .reveal-btn.active:hover {
    color: var(--ui-accent);
  }
</style>
