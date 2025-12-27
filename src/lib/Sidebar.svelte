<!--
  Sidebar.svelte - File list and staging controls
  
  Displays changed files with staged/unstaged state indicators.
  Click row to view unstaged diff. Click staged icon to view staged diff.
  Hover for stage/unstage/discard actions that appear above/below the line.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    CircleFadingArrowUp,
    CircleFadingPlus,
    CircleArrowUp,
    CirclePlus,
    CircleSlash,
    CircleX,
    ArrowLeft,
    ArrowRight,
  } from 'lucide-svelte';
  import { getGitStatus, stageFile, unstageFile, discardFile } from './services/git';
  import type { GitStatus } from './types';
  import HoldToDiscard from './HoldToDiscard.svelte';

  export type FileCategory = 'staged' | 'unstaged' | 'untracked';

  // Hover state for d-pad positioning (fixed position in viewport)
  let hoveredFile: FileEntry | null = $state(null);
  let dpadStyle: {
    lineTop: number;
    lineBottom: number;
    lineLeft: number;
    lineRight: number;
    iconsCenterX: number;
  } | null = $state(null);

  function handleMouseEnter(event: MouseEvent, file: FileEntry) {
    const li = event.currentTarget as HTMLElement;
    const icons = li.querySelector('.state-indicators') as HTMLElement;
    if (!li || !icons) return;

    const lineRect = li.getBoundingClientRect();
    const iconsRect = icons.getBoundingClientRect();

    hoveredFile = file;
    dpadStyle = {
      lineTop: lineRect.top,
      lineBottom: lineRect.bottom,
      lineLeft: lineRect.left,
      lineRight: lineRect.right,
      iconsCenterX: iconsRect.left + iconsRect.width / 2,
    };
  }

  function handleMouseLeave(event: MouseEvent) {
    // Don't clear hover if we're moving to the d-pad overlay
    const relatedTarget = event.relatedTarget as HTMLElement | null;
    if (relatedTarget?.closest('.dpad-overlay')) {
      return;
    }
    hoveredFile = null;
    dpadStyle = null;
  }

  function handleOverlayMouseLeave(event: MouseEvent) {
    // Don't clear hover if we're moving back to the hovered file item
    const relatedTarget = event.relatedTarget as HTMLElement | null;
    if (relatedTarget?.closest('.file-item.is-hovered')) {
      return;
    }
    hoveredFile = null;
    dpadStyle = null;
  }

  interface FileEntry {
    path: string;
    status: string;
    hasStaged: boolean;
    hasUnstaged: boolean;
  }

  interface Props {
    onFileSelect?: (path: string, category: FileCategory) => void;
    onStatusChange?: () => void;
    onRepoLoaded?: (repoPath: string) => void;
    selectedFile?: string | null;
    selectedCategory?: FileCategory | null;
  }

  let {
    onFileSelect,
    onStatusChange,
    onRepoLoaded,
    selectedFile = null,
    selectedCategory = null,
  }: Props = $props();

  let gitStatus: GitStatus | null = $state(null);
  let error: string | null = $state(null);
  let loading = $state(true);

  /**
   * Build unified file list from git status.
   * Each file appears once, with flags for staged/unstaged state.
   */
  function buildFileList(status: GitStatus): FileEntry[] {
    const fileMap = new Map<string, FileEntry>();

    // Add staged files
    for (const f of status.staged) {
      fileMap.set(f.path, {
        path: f.path,
        status: f.status,
        hasStaged: true,
        hasUnstaged: false,
      });
    }

    // Add/update with unstaged files
    for (const f of status.unstaged) {
      const existing = fileMap.get(f.path);
      if (existing) {
        existing.hasUnstaged = true;
      } else {
        fileMap.set(f.path, {
          path: f.path,
          status: f.status,
          hasStaged: false,
          hasUnstaged: true,
        });
      }
    }

    // Add untracked files (treated as unstaged)
    for (const f of status.untracked) {
      fileMap.set(f.path, {
        path: f.path,
        status: f.status,
        hasStaged: false,
        hasUnstaged: true,
      });
    }

    // Sort by path for stable ordering
    return Array.from(fileMap.values()).sort((a, b) => a.path.localeCompare(b.path));
  }

  /**
   * Set status from external source (e.g., watcher events).
   */
  export function setStatus(status: GitStatus) {
    gitStatus = status;
    loading = false;
    error = null;
  }

  let files = $derived(gitStatus ? buildFileList(gitStatus) : []);
  let stagedCount = $derived(files.filter((f) => f.hasStaged).length);
  let totalCount = $derived(files.length);

  onMount(() => {
    loadStatus();
  });

  export async function loadStatus() {
    loading = true;
    error = null;
    try {
      gitStatus = await getGitStatus();

      if (gitStatus?.repo_path) {
        onRepoLoaded?.(gitStatus.repo_path);
      }

      // Auto-select first file if none selected
      if (!selectedFile && gitStatus && onFileSelect) {
        const firstFile = files[0];
        if (firstFile) {
          // Default to unstaged if available, otherwise staged
          const category = firstFile.hasUnstaged ? 'unstaged' : 'staged';
          onFileSelect(firstFile.path, category);
        }
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function selectUnstaged(file: FileEntry) {
    if (file.hasUnstaged) {
      onFileSelect?.(file.path, file.status === 'untracked' ? 'untracked' : 'unstaged');
    } else if (file.hasStaged) {
      // Fall back to staged if no unstaged changes
      onFileSelect?.(file.path, 'staged');
    }
  }

  function selectStaged(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();
    if (file.hasStaged) {
      onFileSelect?.(file.path, 'staged');
    }
  }

  async function handleStage(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();
    try {
      await stageFile(file.path);
      await loadStatus();
      // Switch view to staged
      onFileSelect?.(file.path, 'staged');
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to stage:', e);
    }
  }

  async function handleUnstage(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();
    try {
      await unstageFile(file.path);
      await loadStatus();
      // Switch view to unstaged
      onFileSelect?.(file.path, 'unstaged');
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to unstage:', e);
    }
  }

  async function handleDiscard(file: FileEntry) {
    try {
      // Unstage first if staged, then discard working changes
      if (file.hasStaged) {
        await unstageFile(file.path);
      }
      await discardFile(file.path);

      // Clear hover state since the file is gone
      hoveredFile = null;
      dpadStyle = null;

      const newStatus = await getGitStatus();
      gitStatus = newStatus;

      const newFiles = buildFileList(newStatus);
      if (newFiles.length > 0) {
        const firstFile = newFiles[0];
        const category = firstFile.hasUnstaged ? 'unstaged' : 'staged';
        onFileSelect?.(firstFile.path, category);
      } else {
        onFileSelect?.('', 'unstaged');
      }

      onStatusChange?.();
    } catch (e) {
      console.error('Failed to discard:', e);
    }
  }

  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }

  function getFileDir(path: string): string {
    const parts = path.split('/');
    if (parts.length > 1) {
      return parts.slice(0, -1).join('/') + '/';
    }
    return '';
  }

  function isSelected(file: FileEntry, category: FileCategory): boolean {
    return selectedFile === file.path && selectedCategory === category;
  }

  function isUnstagedSelected(file: FileEntry): boolean {
    return (
      selectedFile === file.path &&
      (selectedCategory === 'unstaged' || selectedCategory === 'untracked')
    );
  }
</script>

<div class="sidebar-content">
  <div class="header">
    <h2>Changes</h2>
    <div class="header-right">
      {#if totalCount > 0}
        <span class="file-counts">
          <span class="staged-count" title="Staged">{stagedCount}</span>
          <span class="separator">/</span>
          <span class="total-count" title="Total">{totalCount}</span>
        </span>
      {/if}
      <button class="refresh-btn" onclick={loadStatus} title="Refresh">↻</button>
    </div>
  </div>

  {#if loading}
    <div class="loading">Loading...</div>
  {:else if error}
    <div class="error">
      <p>Error: {error}</p>
      <button onclick={loadStatus}>Retry</button>
    </div>
  {:else if files.length === 0}
    <div class="empty-state">
      <p>No changes</p>
      <p class="empty-hint">Working tree is clean</p>
    </div>
  {:else}
    <ul class="file-list">
      {#each files as file (file.path)}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
        <li
          class="file-item"
          class:has-selection={selectedFile === file.path}
          class:is-hovered={hoveredFile?.path === file.path}
          onclick={() => selectUnstaged(file)}
          onkeydown={(e) => e.key === 'Enter' && selectUnstaged(file)}
          onmouseenter={(e) => handleMouseEnter(e, file)}
          onmouseleave={handleMouseLeave}
          tabindex="0"
          role="button"
        >
          <!-- State indicators (side by side) -->
          <div class="state-indicators">
            <!-- Staged indicator (clickable) -->
            <button
              class="state-icon staged"
              class:active={file.hasStaged}
              disabled={!file.hasStaged}
              onclick={(e) => selectStaged(e, file)}
              title={file.hasStaged ? 'View staged changes' : ''}
            >
              {#if file.status === 'added' || file.status === 'untracked'}
                <CirclePlus size={14} />
              {:else if file.status === 'deleted'}
                <CircleX size={14} />
              {:else}
                <CircleArrowUp size={14} />
              {/if}
            </button>

            <!-- Unstaged indicator -->
            <div class="state-icon unstaged" class:active={file.hasUnstaged}>
              {#if file.status === 'added' || file.status === 'untracked'}
                <CircleFadingPlus size={14} />
              {:else if file.status === 'deleted'}
                <CircleSlash size={14} />
              {:else}
                <CircleFadingArrowUp size={14} />
              {/if}
            </div>
          </div>

          <!-- File path -->
          <span class="file-path">
            <span class="file-dir">{getFileDir(file.path)}</span>
            <span class="file-name">{getFileName(file.path)}</span>
          </span>
        </li>
      {/each}
    </ul>
  {/if}

  <!-- D-pad overlay - fixed positioned outside the list -->
  {#if hoveredFile && dpadStyle}
    {@const file = hoveredFile}
    {@const pos = dpadStyle}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dpad-overlay" onmouseleave={handleOverlayMouseLeave}>
      <!-- Outline around the row -->
      <div
        class="dpad-outline"
        style="
          top: {pos.lineTop}px;
          left: {pos.lineLeft}px;
          width: {pos.lineRight - pos.lineLeft}px;
          height: {pos.lineBottom - pos.lineTop}px;
        "
      ></div>

      <!-- Stage button (above, centered on icons) -->
      {#if file.hasUnstaged}
        <button
          class="dpad-btn stage-btn"
          style="
            top: {pos.lineTop}px;
            left: {pos.iconsCenterX}px;
            transform: translate(-50%, -100%);
          "
          onclick={(e) => handleStage(e, file)}
          title="Stage"
        >
          <ArrowLeft size={12} />
        </button>
      {/if}

      <!-- Unstage button (below, centered on icons) -->
      {#if file.hasStaged}
        <button
          class="dpad-btn unstage-btn"
          style="
            top: {pos.lineBottom}px;
            left: {pos.iconsCenterX}px;
            transform: translateX(-50%);
          "
          onclick={(e) => handleUnstage(e, file)}
          title="Unstage"
        >
          <ArrowRight size={12} />
        </button>
      {/if}

      <!-- Discard button (left side) - hold to confirm -->
      <div
        class="dpad-discard-wrapper"
        style="
          top: {pos.lineTop}px;
          left: {pos.lineLeft}px;
          height: {pos.lineBottom - pos.lineTop}px;
          transform: translateX(-100%);
        "
      >
        <HoldToDiscard onDiscard={() => handleDiscard(file)} title="Hold to discard" />
      </div>
    </div>
  {/if}
</div>

<style>
  .sidebar-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-primary);
  }

  .header h2 {
    margin: 0;
    font-size: var(--size-md);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-secondary);
  }

  .header-right {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .file-counts {
    font-size: var(--size-sm);
    font-family: monospace;
  }

  .staged-count {
    color: var(--status-added);
  }

  .separator {
    color: var(--text-muted);
  }

  .total-count {
    color: var(--text-muted);
  }

  .refresh-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: var(--size-xl);
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
  }

  .refresh-btn:hover {
    background-color: var(--bg-input);
    color: var(--text-secondary);
  }

  .loading,
  .error,
  .empty-state {
    padding: 20px 16px;
    text-align: center;
    color: var(--text-muted);
  }

  .error {
    color: var(--status-deleted);
  }

  .error button {
    margin-top: 8px;
    padding: 4px 12px;
    background-color: var(--bg-input);
    border: none;
    border-radius: 4px;
    color: var(--text-primary);
    cursor: pointer;
  }

  .empty-state p {
    margin: 0;
  }

  .empty-hint {
    font-size: var(--size-sm);
    margin-top: 4px !important;
  }

  .file-list {
    flex: 1;
    overflow-y: auto;
    list-style: none;
    margin: 0;
    padding: 8px 0;
  }

  .file-item {
    display: flex;
    align-items: center;
    padding: 3px 8px;
    font-size: var(--size-md);
    gap: 6px;
    cursor: pointer;
  }

  .file-item:hover {
    background-color: var(--bg-tertiary);
  }

  .file-item.has-selection {
    background-color: var(--ui-selection);
  }

  /* State indicators (side by side) */
  .state-indicators {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }

  .state-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    color: var(--text-muted);
    opacity: 0.25;
    transition:
      opacity 0.1s,
      color 0.1s;
  }

  .state-icon.active {
    opacity: 1;
  }

  /* Staged icons - solid style, green tint */
  .state-icon.staged {
    background: none;
    border: none;
    padding: 0;
    cursor: default;
  }

  .state-icon.staged.active {
    color: var(--status-added);
    cursor: pointer;
  }

  .state-icon.staged.active:hover {
    color: var(--text-primary);
  }

  .state-icon.staged:disabled {
    cursor: default;
  }

  /* Unstaged icons - fading style, yellow/orange tint */
  .state-icon.unstaged.active {
    color: var(--status-modified);
  }

  .file-path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    direction: rtl;
    text-align: left;
  }

  .file-dir {
    color: var(--text-muted);
  }

  .file-name {
    color: var(--text-primary);
  }

  /* D-pad overlay - fixed positioned in viewport */
  .dpad-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    pointer-events: none;
    z-index: 1000;
  }

  .dpad-outline {
    position: fixed;
    border: 1px solid var(--border-primary);
    border-radius: 0 3px 3px 0;
    pointer-events: none;
  }

  .dpad-btn {
    position: fixed;
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    padding: 2px 6px;
    cursor: pointer;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      color 0.1s,
      background-color 0.1s;
    pointer-events: auto;
  }

  .dpad-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-input);
  }

  /* Stage button - above the line */
  .dpad-btn.stage-btn {
    border-radius: 3px 3px 0 0;
    border-bottom: none;
  }

  .dpad-btn.stage-btn:hover {
    color: var(--status-added);
  }

  /* Unstage button - below the line */
  .dpad-btn.unstage-btn {
    border-radius: 0 0 3px 3px;
    border-top: none;
  }

  .dpad-btn.unstage-btn:hover {
    color: var(--status-modified);
  }

  /* Discard wrapper - positions HoldToDiscard component */
  .dpad-discard-wrapper {
    position: fixed;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: auto;
  }
</style>
