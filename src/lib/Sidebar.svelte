<!--
  Sidebar.svelte - File list with review workflow
  
  Files needing review appear above the line.
  Approved files (staged, no unstaged) appear below the line.
  Hover for approve/discard actions via d-pad overlay.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    Check,
    CircleFadingArrowUp,
    CircleFadingPlus,
    CircleArrowUp,
    CirclePlus,
    CircleMinus,
    CircleX,
    X,
  } from 'lucide-svelte';
  import { getGitStatus, stageFile, discardFile } from './services/git';
  import type { GitStatus } from './types';
  import HoldToDiscard from './HoldToDiscard.svelte';

  export type FileCategory = 'staged' | 'unstaged' | 'untracked';

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
  }

  let { onFileSelect, onStatusChange, onRepoLoaded, selectedFile = null }: Props = $props();

  let gitStatus: GitStatus | null = $state(null);
  let error: string | null = $state(null);
  let loading = $state(true);

  // Hover state for d-pad positioning (fixed position in viewport)
  let hoveredFile: FileEntry | null = $state(null);
  let dpadStyle: {
    lineTop: number;
    lineBottom: number;
    lineLeft: number;
    lineRight: number;
  } | null = $state(null);
  let hoverTimeout: number | null = null;

  function clearHoverState() {
    hoveredFile = null;
    dpadStyle = null;
  }

  function scheduleClearHover() {
    // Small delay to allow mouse to reach the button
    if (hoverTimeout) clearTimeout(hoverTimeout);
    hoverTimeout = window.setTimeout(() => {
      clearHoverState();
      hoverTimeout = null;
    }, 100);
  }

  function cancelClearHover() {
    if (hoverTimeout) {
      clearTimeout(hoverTimeout);
      hoverTimeout = null;
    }
  }

  function handleMouseEnter(event: MouseEvent, file: FileEntry) {
    cancelClearHover();
    const li = event.currentTarget as HTMLElement;
    if (!li) return;

    const lineRect = li.getBoundingClientRect();

    hoveredFile = file;
    dpadStyle = {
      lineTop: lineRect.top,
      lineBottom: lineRect.bottom,
      lineLeft: lineRect.left,
      lineRight: lineRect.right,
    };
  }

  function handleMouseLeave() {
    scheduleClearHover();
  }

  function handleOverlayMouseLeave() {
    scheduleClearHover();
  }

  function handleButtonMouseEnter() {
    cancelClearHover();
  }

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

  // Split into needs review (has unstaged) and approved (staged only)
  let needsReview = $derived(files.filter((f) => f.hasUnstaged));
  let approved = $derived(files.filter((f) => f.hasStaged && !f.hasUnstaged));

  let approvedCount = $derived(approved.length);
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
        const firstFile = needsReview[0] || approved[0];
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

  /**
   * Select a file - shows unstaged diff if available, else staged.
   */
  function selectFile(file: FileEntry) {
    if (file.hasUnstaged) {
      onFileSelect?.(file.path, file.status === 'untracked' ? 'untracked' : 'unstaged');
    } else if (file.hasStaged) {
      onFileSelect?.(file.path, 'staged');
    }
  }

  async function handleApprove(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();
    try {
      await stageFile(file.path);
      await loadStatus();
      // Keep file selected, view will update
      const updatedFile = files.find((f) => f.path === file.path);
      if (updatedFile) {
        selectFile(updatedFile);
      }
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to approve:', e);
    }
  }

  async function handleDiscard(file: FileEntry) {
    try {
      await discardFile(file.path);

      // Clear hover state since the file may be gone
      hoveredFile = null;
      dpadStyle = null;

      const newStatus = await getGitStatus();
      gitStatus = newStatus;

      const newFiles = buildFileList(newStatus);
      if (newFiles.length > 0) {
        const firstFile = newFiles.filter((f) => f.hasUnstaged)[0] || newFiles[0];
        selectFile(firstFile);
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
</script>

<div class="sidebar-content">
  <div class="header">
    <h2>Changes</h2>
    <div class="header-right">
      {#if totalCount > 0}
        <span class="file-counts">
          <span class="approved-count" title="Approved">{approvedCount}</span>
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
    <div class="file-list">
      <!-- Needs Review section -->
      {#if needsReview.length > 0}
        <ul class="file-section">
          {#each needsReview as file (file.path)}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <li
              class="file-item"
              class:selected={selectedFile === file.path}
              class:is-hovered={hoveredFile?.path === file.path}
              onclick={() => selectFile(file)}
              onkeydown={(e) => e.key === 'Enter' && selectFile(file)}
              onmouseenter={(e) => handleMouseEnter(e, file)}
              onmouseleave={handleMouseLeave}
              tabindex="0"
              role="button"
            >
              <!-- Status icon - fading if no checkpoint, solid if has checkpoint -->
              <span class="status-icon" class:has-checkpoint={file.hasStaged}>
                {#if file.hasStaged}
                  <!-- Solid icons for files with checkpoint -->
                  {#if file.status === 'added' || file.status === 'untracked'}
                    <CirclePlus size={16} />
                  {:else if file.status === 'deleted'}
                    <CircleMinus size={16} />
                  {:else}
                    <CircleArrowUp size={16} />
                  {/if}
                {:else}
                  <!-- Fading icons for files without checkpoint -->
                  {#if file.status === 'added' || file.status === 'untracked'}
                    <CircleFadingPlus size={16} />
                  {:else if file.status === 'deleted'}
                    <CircleX size={16} />
                  {:else}
                    <CircleFadingArrowUp size={16} />
                  {/if}
                {/if}
              </span>

              <!-- File path -->
              <span class="file-path">
                <span class="file-dir">{getFileDir(file.path)}</span>
                <span class="file-name">{getFileName(file.path)}</span>
              </span>
            </li>
          {/each}
        </ul>
      {/if}

      <!-- Separator -->
      {#if needsReview.length > 0 && approved.length > 0}
        <div class="section-divider"></div>
      {/if}

      <!-- Approved section -->
      {#if approved.length > 0}
        <ul class="file-section approved-section">
          {#each approved as file (file.path)}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <li
              class="file-item approved"
              class:selected={selectedFile === file.path}
              onclick={() => selectFile(file)}
              onkeydown={(e) => e.key === 'Enter' && selectFile(file)}
              tabindex="0"
              role="button"
            >
              <!-- Status icon - solid (approved/checkpointed) -->
              <span class="status-icon has-checkpoint">
                {#if file.status === 'added' || file.status === 'untracked'}
                  <CirclePlus size={16} />
                {:else if file.status === 'deleted'}
                  <CircleMinus size={16} />
                {:else}
                  <CircleArrowUp size={16} />
                {/if}
              </span>

              <span class="file-path">
                <span class="file-dir">{getFileDir(file.path)}</span>
                <span class="file-name">{getFileName(file.path)}</span>
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
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

      <!-- Approve button (left side, aligned with sidebar border) -->
      <button
        class="dpad-btn approve-btn"
        style="
          top: {pos.lineTop}px;
          left: {pos.lineLeft - 1}px;
          height: {pos.lineBottom - pos.lineTop}px;
          transform: translateX(-100%);
        "
        onclick={(e) => handleApprove(e, file)}
        onmouseenter={handleButtonMouseEnter}
        title="Approve"
      >
        <Check size={12} />
      </button>

      <!-- Discard button (right side, inside row) - hold to confirm -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="dpad-discard-wrapper"
        style="
          top: {pos.lineTop}px;
          left: {pos.lineLeft}px;
          width: {pos.lineRight - pos.lineLeft}px;
          height: {pos.lineBottom - pos.lineTop}px;
        "
        onmouseenter={handleButtonMouseEnter}
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

  .approved-count {
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
    padding: 8px 0;
  }

  .file-section {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .section-divider {
    height: 1px;
    background: var(--border-primary);
    margin: 4px 8px;
  }

  .approved-section {
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

  .file-item:hover,
  .file-item.is-hovered {
    background-color: var(--bg-tertiary);
  }

  .file-item.selected {
    background-color: var(--ui-selection);
  }

  /* Status icon */
  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  /* Has checkpoint + needs review = yellow (modified color) */
  .status-icon.has-checkpoint {
    color: var(--status-modified);
  }

  /* Approved section = green (fully approved) */
  .approved-section .status-icon.has-checkpoint {
    color: var(--status-added);
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
    border-left: none;
    border-radius: 0;
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

  /* Approve button - left side */
  .dpad-btn.approve-btn {
    border-radius: 3px 0 0 3px;
    border-right: none;
  }

  .dpad-btn.approve-btn:hover {
    color: var(--status-added);
  }

  /* Discard wrapper - positions HoldToDiscard component flush to right edge */
  .dpad-discard-wrapper {
    position: fixed;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    pointer-events: none;
  }

  /* Only the button itself should capture pointer events */
  :global(.dpad-discard-wrapper .hold-to-discard) {
    pointer-events: auto;
  }
</style>
