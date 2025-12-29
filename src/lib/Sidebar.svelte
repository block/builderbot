<!--
  Sidebar.svelte - File list with review workflow
  
  Shows files changed in the current diff (base..head).
  Files needing review appear above the divider.
  Reviewed files appear below the divider.
  Review state comes from the review storage, not git index.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    MessageSquare,
    CircleFadingArrowUp,
    CircleFadingPlus,
    CircleArrowUp,
    CirclePlus,
    CircleMinus,
    CircleX,
    Plus,
    Minus,
    Trash2,
    Check,
    RotateCcw,
  } from 'lucide-svelte';
  import {
    getGitStatus,
    getChangedFiles,
    stageFile,
    unstageFile,
    discardFile,
  } from './services/git';
  import { getReview, markReviewed, unmarkReviewed } from './services/review';
  import type { GitStatus, Review, ChangedFile } from './types';

  interface FileEntry {
    path: string;
    status: string;
    isReviewed: boolean;
    commentCount: number;
    // Only populated when viewing working tree (diffHead === "@")
    hasStaged?: boolean;
    hasUnstaged?: boolean;
  }

  interface Props {
    /** Called when user selects a file to view */
    onFileSelect?: (path: string) => void;
    /** Called when staging/unstaging/discarding changes */
    onStatusChange?: () => void;
    /** Called when repo is loaded */
    onRepoLoaded?: (repoPath: string) => void;
    /** Currently selected file path */
    selectedFile?: string | null;
    /** Base ref for the diff (controlled by parent) */
    diffBase?: string;
    /** Head ref for the diff (controlled by parent) */
    diffHead?: string;
  }

  let {
    onFileSelect,
    onStatusChange,
    onRepoLoaded,
    selectedFile = null,
    diffBase = 'HEAD',
    diffHead = '@',
  }: Props = $props();

  let changedFiles: ChangedFile[] = $state([]);
  let review: Review | null = $state(null);
  let error: string | null = $state(null);
  let loading = $state(true);

  // Context menu state - only show for working tree diffs
  let contextMenu: { x: number; y: number; file: FileEntry } | null = $state(null);
  let holdingDiscard = $state(false);
  let discardProgress = $state(0);
  let discardStartTime: number | null = null;
  let discardAnimationFrame: number | null = null;
  const HOLD_DURATION = 700;

  // Can we modify files? Only when viewing working tree
  let canModify = $derived(diffHead === '@');

  /**
   * Build file list with review state.
   */
  function buildFileList(files: ChangedFile[], reviewData: Review | null): FileEntry[] {
    const reviewedSet = new Set(reviewData?.reviewed || []);

    // Count comments per file
    const commentCounts = new Map<string, number>();
    for (const comment of reviewData?.comments || []) {
      commentCounts.set(comment.file_path, (commentCounts.get(comment.file_path) || 0) + 1);
    }

    return files.map((f) => ({
      path: f.path,
      status: f.status,
      isReviewed: reviewedSet.has(f.path),
      commentCount: commentCounts.get(f.path) || 0,
    }));
  }

  /**
   * Set status from external source (e.g., watcher events).
   * Only relevant when viewing working tree.
   */
  export function setStatus(_status: GitStatus) {
    // When watcher fires, reload the file list
    if (diffHead === '@') {
      loadStatus();
    }
  }

  let files = $derived(buildFileList(changedFiles, review));
  let needsReview = $derived(files.filter((f) => !f.isReviewed));
  let reviewed = $derived(files.filter((f) => f.isReviewed));
  let reviewedCount = $derived(reviewed.length);
  let totalCount = $derived(files.length);

  onMount(() => {
    loadStatus();

    // Close context menu on click outside
    const handleClickOutside = () => {
      if (contextMenu) {
        closeContextMenu();
      }
    };
    window.addEventListener('click', handleClickOutside);
    return () => window.removeEventListener('click', handleClickOutside);
  });

  export async function loadStatus() {
    // Only show loading state on initial load
    const isInitialLoad = changedFiles.length === 0 && !error;
    if (isInitialLoad) {
      loading = true;
    }
    error = null;

    try {
      // Get changed files for the current diff
      changedFiles = await getChangedFiles(diffBase, diffHead);

      // Load review state for current diff
      review = await getReview(diffBase, diffHead);

      // Get repo path for watcher (only need this once)
      const status = await getGitStatus();
      if (status?.repo_path) {
        onRepoLoaded?.(status.repo_path);
      }

      // Auto-select first file if none selected
      if (!selectedFile && changedFiles.length > 0 && onFileSelect) {
        const allFiles = buildFileList(changedFiles, review);
        const firstFile = allFiles.filter((f) => !f.isReviewed)[0] || allFiles[0];
        if (firstFile) {
          selectFile(firstFile);
        }
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function selectFile(file: FileEntry) {
    onFileSelect?.(file.path);
  }

  async function toggleReviewed(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();
    try {
      if (file.isReviewed) {
        await unmarkReviewed(diffBase, diffHead, file.path);
      } else {
        await markReviewed(diffBase, diffHead, file.path);
      }
      // Reload review state
      review = await getReview(diffBase, diffHead);
    } catch (e) {
      console.error('Failed to toggle reviewed:', e);
    }
  }

  // Context menu handlers - only for working tree mode
  function handleContextMenu(event: MouseEvent, file: FileEntry) {
    if (!canModify) return; // No context menu for historical diffs
    event.preventDefault();
    event.stopPropagation();
    contextMenu = { x: event.clientX, y: event.clientY, file };
  }

  function closeContextMenu() {
    contextMenu = null;
    cancelDiscardHold();
  }

  async function handleStage(file: FileEntry) {
    try {
      await stageFile(file.path);
      await loadStatus();
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to stage:', e);
    }
    closeContextMenu();
  }

  async function handleUnstage(file: FileEntry) {
    try {
      await unstageFile(file.path);
      await loadStatus();
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to unstage:', e);
    }
    closeContextMenu();
  }

  async function handleDiscard(file: FileEntry) {
    try {
      await discardFile(file.path);
      await loadStatus();

      // Select next file if available
      if (files.length > 0) {
        const nextFile = needsReview[0] || reviewed[0];
        if (nextFile) {
          selectFile(nextFile);
        }
      } else {
        // No files left - clear selection
        onFileSelect?.('');
      }

      onStatusChange?.();
    } catch (e) {
      console.error('Failed to discard:', e);
    }
    closeContextMenu();
  }

  // Hold-to-discard logic for context menu
  function startDiscardHold() {
    holdingDiscard = true;
    discardProgress = 0;
    discardStartTime = Date.now();
    discardAnimationFrame = requestAnimationFrame(updateDiscardProgress);
  }

  function updateDiscardProgress() {
    if (!discardStartTime || !contextMenu) return;

    const elapsed = Date.now() - discardStartTime;
    discardProgress = Math.min(elapsed / HOLD_DURATION, 1);

    if (discardProgress >= 1) {
      handleDiscard(contextMenu.file);
    } else {
      discardAnimationFrame = requestAnimationFrame(updateDiscardProgress);
    }
  }

  function cancelDiscardHold() {
    holdingDiscard = false;
    discardProgress = 0;
    discardStartTime = null;
    if (discardAnimationFrame) {
      cancelAnimationFrame(discardAnimationFrame);
      discardAnimationFrame = null;
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
</script>

<div class="sidebar-content">
  <div class="header">
    {#if totalCount > 0}
      <span class="file-counts">{reviewedCount}/{totalCount} reviewed</span>
    {:else}
      <span class="file-counts">Files</span>
    {/if}
    <button class="refresh-btn" onclick={loadStatus} title="Refresh">↻</button>
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
      {#if canModify}
        <p class="empty-hint">Working tree is clean</p>
      {:else}
        <p class="empty-hint">No differences between refs</p>
      {/if}
    </div>
  {:else}
    <div class="file-list">
      <!-- Needs Review section -->
      {#if needsReview.length > 0}
        <ul class="file-section">
          {#each needsReview as file (file.path)}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <li
              class="file-item"
              class:selected={selectedFile === file.path}
              onclick={() => selectFile(file)}
              oncontextmenu={(e) => handleContextMenu(e, file)}
              tabindex="0"
              role="button"
            >
              <!-- Status icon - clickable to toggle reviewed -->
              <button
                class="status-icon"
                onclick={(e) => toggleReviewed(e, file)}
                title="Mark as reviewed"
              >
                <!-- Default icon (hidden on hover) -->
                <span class="icon-default">
                  {#if file.status === 'added' || file.status === 'untracked'}
                    {#if canModify}
                      <CircleFadingPlus size={16} />
                    {:else}
                      <CirclePlus size={16} />
                    {/if}
                  {:else if file.status === 'deleted'}
                    {#if canModify}
                      <CircleX size={16} />
                    {:else}
                      <CircleMinus size={16} />
                    {/if}
                  {:else if canModify}
                    <CircleFadingArrowUp size={16} />
                  {:else}
                    <CircleArrowUp size={16} />
                  {/if}
                </span>
                <!-- Hover icon (checkmark for "mark as reviewed") -->
                <span class="icon-hover">
                  <Check size={16} />
                </span>
              </button>

              <!-- File path -->
              <span class="file-path">
                <span class="file-dir">{getFileDir(file.path)}</span>
                <span class="file-name">{getFileName(file.path)}</span>
              </span>

              <!-- Comment indicator -->
              {#if file.commentCount > 0}
                <span class="comment-indicator">
                  <MessageSquare size={12} />
                  <span class="comment-count">{file.commentCount}</span>
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      <!-- Divider with REVIEWED label -->
      {#if reviewed.length > 0}
        <div class="section-divider">
          <span class="divider-label">REVIEWED</span>
        </div>
      {/if}

      <!-- Reviewed section -->
      {#if reviewed.length > 0}
        <ul class="file-section reviewed-section">
          {#each reviewed as file (file.path)}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <li
              class="file-item"
              class:selected={selectedFile === file.path}
              onclick={() => selectFile(file)}
              oncontextmenu={(e) => handleContextMenu(e, file)}
              tabindex="0"
              role="button"
            >
              <!-- Status icon - clickable to toggle reviewed -->
              <button
                class="status-icon"
                onclick={(e) => toggleReviewed(e, file)}
                title="Mark as needs review"
              >
                <!-- Default icon (hidden on hover) -->
                <span class="icon-default">
                  {#if file.status === 'added' || file.status === 'untracked'}
                    {#if canModify}
                      <CircleFadingPlus size={16} />
                    {:else}
                      <CirclePlus size={16} />
                    {/if}
                  {:else if file.status === 'deleted'}
                    {#if canModify}
                      <CircleX size={16} />
                    {:else}
                      <CircleMinus size={16} />
                    {/if}
                  {:else if canModify}
                    <CircleFadingArrowUp size={16} />
                  {:else}
                    <CircleArrowUp size={16} />
                  {/if}
                </span>
                <!-- Hover icon (rotate for "unmark as reviewed") -->
                <span class="icon-hover icon-hover-unreview">
                  <RotateCcw size={16} />
                </span>
              </button>

              <span class="file-path">
                <span class="file-dir">{getFileDir(file.path)}</span>
                <span class="file-name">{getFileName(file.path)}</span>
              </span>

              {#if file.commentCount > 0}
                <span class="comment-indicator">
                  <MessageSquare size={12} />
                  <span class="comment-count">{file.commentCount}</span>
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <!-- Context Menu -->
  {#if contextMenu}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="context-menu"
      style="left: {contextMenu.x}px; top: {contextMenu.y}px;"
      onclick={(e) => e.stopPropagation()}
    >
      {#if contextMenu.file.hasUnstaged}
        <button class="context-item" onclick={() => handleStage(contextMenu!.file)}>
          <Plus size={14} />
          <span>Stage</span>
        </button>
      {/if}
      {#if contextMenu.file.hasStaged}
        <button class="context-item" onclick={() => handleUnstage(contextMenu!.file)}>
          <Minus size={14} />
          <span>Unstage</span>
        </button>
      {/if}
      <div class="context-divider"></div>
      <button
        class="context-item discard-item"
        onmousedown={startDiscardHold}
        onmouseup={cancelDiscardHold}
        onmouseleave={cancelDiscardHold}
      >
        <span class="discard-progress" style="width: {discardProgress * 100}%"></span>
        <Trash2 size={14} />
        <span>Discard</span>
      </button>
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
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-primary);
    gap: 8px;
  }

  .file-counts {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .refresh-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: var(--size-lg);
    cursor: pointer;
    padding: 2px 4px;
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
    padding: 8px 0;
  }

  .file-section {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  /* Divider with REVIEWED label */
  .section-divider {
    display: flex;
    align-items: center;
    margin: 8px 12px;
    gap: 8px;
  }

  .section-divider::before,
  .section-divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-primary);
  }

  .divider-label {
    font-size: 9px;
    font-weight: 500;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    text-transform: uppercase;
  }

  .reviewed-section {
    opacity: 0.7;
  }

  .file-item {
    display: flex;
    align-items: center;
    padding: 3px 8px;
    font-size: var(--size-md);
    gap: 6px;
    cursor: pointer;
    position: relative;
  }

  .file-item:hover {
    background-color: var(--bg-tertiary);
  }

  .file-item.selected {
    background-color: var(--ui-selection);
  }

  /* Status icon as button */
  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: none;
    border: none;
    padding: 2px;
    margin: -2px;
    cursor: pointer;
    color: var(--text-muted);
    border-radius: 3px;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .status-icon:hover {
    background-color: var(--bg-input);
    color: var(--status-added);
  }

  /* Icon swap on hover */
  .icon-default,
  .icon-hover {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .icon-hover {
    display: none;
  }

  .status-icon:hover .icon-default {
    display: none;
  }

  .status-icon:hover .icon-hover {
    display: flex;
  }

  /* Unreview hover icon uses muted color instead of green */
  .status-icon:hover .icon-hover-unreview {
    color: var(--text-muted);
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

  /* Comment indicator */
  .comment-indicator {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--text-muted);
    font-size: 10px;
    flex-shrink: 0;
  }

  .comment-count {
    font-family: monospace;
  }

  /* Context Menu */
  .context-menu {
    position: fixed;
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-radius: 6px;
    padding: 4px 0;
    min-width: 160px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    z-index: 1000;
  }

  .context-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    position: relative;
    overflow: hidden;
  }

  .context-item:hover {
    background-color: var(--bg-tertiary);
  }

  .context-divider {
    height: 1px;
    background: var(--border-primary);
    margin: 4px 0;
  }

  /* Hold to discard - no red text, just red progress bar */
  .context-item.discard-item {
    color: var(--text-primary);
  }

  .context-item.discard-item:hover {
    background-color: var(--bg-tertiary);
  }

  .discard-progress {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    background-color: var(--status-deleted);
    opacity: 0.5;
    pointer-events: none;
  }
</style>
