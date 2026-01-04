<script lang="ts">
  import {
    ChevronDown,
    Palette,
    MessageSquare,
    Copy,
    Check,
    Trash2,
    FolderGit2,
    Settings2,
    GitCompareArrows,
    FolderOpen,
    X,
  } from 'lucide-svelte';
  import DiffSelectorModal from './DiffSelectorModal.svelte';
  import type { DiffSpec } from './types';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
  } from './stores/preferences.svelte';
  import { getPresets, diffSelection } from './stores/diffSelection.svelte';
  import {
    commentsState,
    copyCommentsToClipboard,
    deleteAllComments,
  } from './stores/comments.svelte';
  import {
    repoState,
    openRepoPicker,
    openRepo,
    removeFromRecent,
    type RepoEntry,
  } from './stores/repoState.svelte';

  interface Props {
    onDiffSelect: (spec: DiffSpec) => void;
    onCustomDiff: (base: string, head: string) => void;
    onRepoChange?: () => void;
  }

  let { onDiffSelect, onCustomDiff, onRepoChange }: Props = $props();

  // Dropdown states
  let diffDropdownOpen = $state(false);
  let themeDropdownOpen = $state(false);
  let repoDropdownOpen = $state(false);

  // Modal state
  let showCustomModal = $state(false);

  // Copy feedback
  let copiedFeedback = $state(false);

  // Check if current selection matches a preset
  function isPresetSelected(preset: DiffSpec): boolean {
    return preset.base === diffSelection.spec.base && preset.head === diffSelection.spec.head;
  }

  // Get current display label
  let currentLabel = $derived.by(() => {
    const presets = getPresets();
    const match = presets.find(
      (p) => p.base === diffSelection.spec.base && p.head === diffSelection.spec.head
    );
    return match?.label ?? `${diffSelection.spec.base}..${diffSelection.spec.head}`;
  });

  function handlePresetSelect(preset: DiffSpec) {
    diffDropdownOpen = false;
    onDiffSelect(preset);
  }

  function handleCustomClick() {
    diffDropdownOpen = false;
    showCustomModal = true;
  }

  function handleCustomSubmit(base: string, head: string) {
    showCustomModal = false;
    onCustomDiff(base, head);
  }

  function handleThemeSelect(theme: string) {
    themeDropdownOpen = false;
    selectSyntaxTheme(theme as any);
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

  // Repo selection handlers
  async function handleOpenRepo() {
    repoDropdownOpen = false;
    const success = await openRepoPicker();
    if (success) {
      onRepoChange?.();
    }
  }

  async function handleRecentRepoSelect(entry: RepoEntry) {
    repoDropdownOpen = false;
    const success = await openRepo(entry.path);
    if (success) {
      onRepoChange?.();
    }
  }

  function handleRemoveRecent(event: MouseEvent, path: string) {
    event.stopPropagation();
    removeFromRecent(path);
  }

  /**
   * Shorten a path by replacing home directory with ~
   */
  function shortenPath(path: string): string {
    // Try to detect home directory and replace with ~
    const homeDir = path.match(/^(\/Users\/[^/]+|\/home\/[^/]+)/)?.[0];
    if (homeDir) {
      return path.replace(homeDir, '~');
    }
    return path;
  }

  // Close dropdowns when clicking outside
  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest('.repo-selector-container')) {
      repoDropdownOpen = false;
    }
    if (!target.closest('.diff-selector')) {
      diffDropdownOpen = false;
    }
    if (!target.closest('.theme-picker')) {
      themeDropdownOpen = false;
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

<header class="top-bar">
  <!-- Left section: Repo selector + Diff selector -->
  <div class="section section-left">
    <div class="repo-selector-container">
      <button
        class="repo-selector"
        onclick={() => (repoDropdownOpen = !repoDropdownOpen)}
        class:open={repoDropdownOpen}
        title="Select repository"
      >
        <FolderGit2 size={14} />
        <span class="repo-name">{repoState.currentName}</span>
        <ChevronDown size={12} />
      </button>

      {#if repoDropdownOpen}
        <div class="dropdown repo-dropdown">
          {#if repoState.recentRepos.length > 0}
            {#each repoState.recentRepos as entry (entry.path)}
              <div
                class="dropdown-item repo-item"
                class:active={entry.path === repoState.currentPath}
                role="button"
                tabindex="0"
                onclick={() => handleRecentRepoSelect(entry)}
                onkeydown={(e) => e.key === 'Enter' && handleRecentRepoSelect(entry)}
              >
                <FolderGit2 size={14} />
                <div class="repo-item-content">
                  <span class="repo-item-name">{entry.name}</span>
                  <span class="repo-item-path">{shortenPath(entry.path)}</span>
                </div>
                <button
                  class="remove-btn"
                  onclick={(e) => handleRemoveRecent(e, entry.path)}
                  title="Remove from recent"
                >
                  <X size={14} />
                </button>
              </div>
            {/each}
            <div class="dropdown-divider"></div>
          {/if}
          <button class="dropdown-item open-item" onclick={handleOpenRepo}>
            <FolderOpen size={12} />
            <span>Open...</span>
          </button>
        </div>
      {/if}
    </div>

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
                <span class="diff-item-spec">{preset.base}..{preset.head}</span>
              </div>
            </button>
          {/each}
          <div class="dropdown-divider"></div>
          <button class="dropdown-item custom-item" onclick={handleCustomClick}>
            <Settings2 size={12} />
            <span>Custom range...</span>
          </button>
        </div>
      {/if}
    </div>
  </div>

  <!-- Center section: Comments -->
  <div class="section section-center">
    <div class="comments-section">
      <MessageSquare size={14} />
      <span class="comment-count">{commentsState.comments.length}</span>
      {#if commentsState.comments.length > 0}
        <button
          class="icon-btn"
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
        <button class="icon-btn delete-btn" onclick={deleteAllComments} title="Delete all comments">
          <Trash2 size={12} />
        </button>
      {/if}
    </div>
  </div>

  <!-- Right section: Theme -->
  <div class="section section-right">
    <div class="theme-picker">
      <button
        class="icon-btn theme-btn"
        onclick={() => (themeDropdownOpen = !themeDropdownOpen)}
        title="Syntax theme"
        class:open={themeDropdownOpen}
      >
        <Palette size={14} />
      </button>

      {#if themeDropdownOpen}
        <div class="dropdown theme-dropdown">
          {#each getAvailableSyntaxThemes() as theme}
            <button
              class="dropdown-item"
              class:active={theme === preferences.syntaxTheme}
              onclick={() => handleThemeSelect(theme)}
            >
              {theme}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</header>

{#if showCustomModal}
  <DiffSelectorModal
    initialBase={diffSelection.spec.base}
    initialHead={diffSelection.spec.head}
    onSubmit={handleCustomSubmit}
    onClose={() => (showCustomModal = false)}
  />
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

  /* Repo selector */
  .repo-selector-container {
    position: relative;
  }

  .repo-selector {
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
    max-width: 200px;
  }

  .repo-selector:hover,
  .repo-selector.open {
    background: var(--bg-hover);
  }

  .repo-selector :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .repo-selector :global(svg:last-child) {
    transition: transform 0.15s;
  }

  .repo-selector.open :global(svg:last-child) {
    transform: rotate(180deg);
  }

  .repo-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-dropdown {
    left: 0;
    width: 290px;
    padding-bottom: 4px;
  }

  .repo-item {
    position: relative;
    padding-right: 32px;
    align-items: flex-start;
  }

  .repo-item :global(svg) {
    flex-shrink: 0;
    margin-top: 2px;
  }

  .repo-item-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .repo-item-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-item-path {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    unicode-bidi: plaintext;
  }

  .remove-btn {
    position: absolute;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s;
  }

  .repo-item:hover .remove-btn {
    opacity: 1;
  }

  .remove-btn:hover {
    color: var(--status-deleted);
  }

  .open-item {
    color: var(--text-muted);
  }

  .open-item :global(svg) {
    color: var(--text-muted);
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

  .theme-dropdown {
    right: 0;
    min-width: 210px;
    max-height: 360px;
    overflow-y: auto;
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
</style>
