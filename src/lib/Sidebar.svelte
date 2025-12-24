<script lang="ts">
  import { onMount } from 'svelte';
  import { getGitStatus, stageFile, unstageFile, discardFile } from './services/git';
  import type { GitStatus } from './types';
  import { ask } from '@tauri-apps/plugin-dialog';

  export type FileCategory = 'staged' | 'unstaged' | 'untracked';

  interface FileEntry {
    path: string;
    status: string;
    staged: boolean;
    untracked: boolean;
  }

  interface Props {
    onFileSelect?: (path: string, category: FileCategory) => void;
    onStatusChange?: () => void;
    selectedFile?: string | null;
  }

  let { onFileSelect, onStatusChange, selectedFile = null }: Props = $props();

  let gitStatus: GitStatus | null = $state(null);
  let error: string | null = $state(null);
  let loading = $state(true);

  // Unified file list - combines all categories, sorted by path
  let files = $derived.by(() => {
    if (!gitStatus) return [];

    const entries: FileEntry[] = [];

    // Add staged files
    for (const f of gitStatus.staged) {
      entries.push({ path: f.path, status: f.status, staged: true, untracked: false });
    }

    // Add unstaged files (may overlap with staged if partially staged)
    for (const f of gitStatus.unstaged) {
      // Check if already in list (file can be both staged and unstaged)
      const existing = entries.find((e) => e.path === f.path);
      if (!existing) {
        entries.push({ path: f.path, status: f.status, staged: false, untracked: false });
      }
    }

    // Add untracked files
    for (const f of gitStatus.untracked) {
      entries.push({ path: f.path, status: f.status, staged: false, untracked: true });
    }

    // Sort by path for stable ordering
    return entries.sort((a, b) => a.path.localeCompare(b.path));
  });

  let stagedCount = $derived(files.filter((f) => f.staged).length);
  let totalCount = $derived(files.length);

  onMount(() => {
    loadStatus();
  });

  export async function loadStatus() {
    loading = true;
    error = null;
    try {
      gitStatus = await getGitStatus();
      // Auto-select first file if none selected
      if (!selectedFile && gitStatus && onFileSelect) {
        const firstFile = files[0];
        if (firstFile) {
          const category = getCategory(firstFile);
          onFileSelect(firstFile.path, category);
        }
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function getCategory(file: FileEntry): FileCategory {
    if (file.staged) return 'staged';
    if (file.untracked) return 'untracked';
    return 'unstaged';
  }

  function selectFile(file: FileEntry) {
    onFileSelect?.(file.path, getCategory(file));
  }

  async function handleToggleStage(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();

    try {
      if (file.staged) {
        await unstageFile(file.path);
      } else {
        await stageFile(file.path);
      }
      await loadStatus();

      // Update selection - file stays in place, just category changes
      const newCategory: FileCategory = file.staged ? 'unstaged' : 'staged';
      onFileSelect?.(file.path, newCategory);
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to toggle stage:', e);
    }
  }

  async function handleDiscard(event: MouseEvent, file: FileEntry) {
    event.stopPropagation();

    const confirmed = await ask(
      `Discard all changes to ${file.path.split('/').pop()}? This cannot be undone.`,
      {
        title: 'Discard Changes',
        kind: 'warning',
        okLabel: 'Discard',
        cancelLabel: 'Cancel',
      }
    );
    if (!confirmed) return;

    try {
      // If staged, unstage first then discard
      if (file.staged) {
        await unstageFile(file.path);
      }
      await discardFile(file.path);
      await loadStatus();
      onStatusChange?.();
    } catch (e) {
      console.error('Failed to discard:', e);
    }
  }

  function getStatusIcon(status: string): string {
    switch (status) {
      case 'modified':
        return 'M';
      case 'added':
        return 'A';
      case 'deleted':
        return 'D';
      case 'renamed':
        return 'R';
      case 'untracked':
        return '?';
      default:
        return '•';
    }
  }

  function getStatusColor(status: string): string {
    switch (status) {
      case 'modified':
        return 'var(--status-modified)';
      case 'added':
        return 'var(--status-added)';
      case 'deleted':
        return 'var(--status-deleted)';
      case 'renamed':
        return 'var(--status-renamed)';
      case 'untracked':
        return 'var(--status-untracked)';
      default:
        return 'var(--text-primary)';
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
        <li
          class="file-item"
          class:selected={selectedFile === file.path}
          onclick={() => selectFile(file)}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === 'Enter' && selectFile(file)}
        >
          <button
            class="stage-checkbox"
            class:checked={file.staged}
            onclick={(e) => handleToggleStage(e, file)}
            title={file.staged ? 'Unstage file' : 'Stage file'}
          >
            {#if file.staged}✓{/if}
          </button>
          <span class="status-icon" style="color: {getStatusColor(file.status)}"
            >{getStatusIcon(file.status)}</span
          >
          <span class="file-path">
            <span class="file-dir">{getFileDir(file.path)}</span>
            <span class="file-name">{getFileName(file.path)}</span>
          </span>
          <button
            class="discard-btn"
            onclick={(e) => handleDiscard(e, file)}
            title={file.untracked ? 'Delete file' : 'Discard changes'}
          >
            ×
          </button>
        </li>
      {/each}
    </ul>
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
    font-size: 13px;
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
    font-size: 12px;
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
    font-size: 16px;
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
    font-size: 12px;
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
    padding: 3px 8px 3px 12px;
    cursor: pointer;
    font-size: 13px;
    gap: 6px;
  }

  .file-item:hover {
    background-color: var(--bg-tertiary);
  }

  .file-item.selected {
    background-color: var(--ui-selection);
  }

  .stage-checkbox {
    width: 16px;
    height: 16px;
    border: 1px solid var(--border-primary);
    border-radius: 3px;
    background: transparent;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    color: transparent;
    flex-shrink: 0;
    transition: all 0.1s ease;
  }

  .stage-checkbox:hover {
    border-color: var(--ui-accent);
    background-color: var(--bg-input);
  }

  .stage-checkbox.checked {
    background-color: var(--ui-accent);
    border-color: var(--ui-accent);
    color: white;
  }

  .stage-checkbox.checked:hover {
    background-color: var(--ui-accent-hover);
  }

  .status-icon {
    width: 14px;
    font-family: monospace;
    font-weight: bold;
    font-size: 12px;
    flex-shrink: 0;
    text-align: center;
  }

  .file-path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .file-dir {
    color: var(--text-muted);
  }

  .file-name {
    color: var(--text-primary);
  }

  .discard-btn {
    width: 20px;
    height: 20px;
    border: none;
    border-radius: 3px;
    background: transparent;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 16px;
    color: var(--text-muted);
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.1s ease;
  }

  .file-item:hover .discard-btn {
    opacity: 1;
  }

  .discard-btn:hover {
    background-color: var(--status-deleted);
    color: white;
  }
</style>
