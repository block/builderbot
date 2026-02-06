<script lang="ts">
  import { X, AlertCircle, Check, GitCommitHorizontal } from 'lucide-svelte';
  import { commit } from './services/git';
  import { diffState } from './stores/diffState.svelte';
  import type { FileDiffSummary } from './types';

  interface Props {
    repoPath: string | null;
    onCommit: () => void;
    onClose: () => void;
  }

  let { repoPath, onCommit, onClose }: Props = $props();

  let message = $state('');
  let selectedPaths = $state<Set<string>>(new Set());
  let error = $state<string | null>(null);
  let committing = $state(false);

  // Get files from diffState
  let files = $derived(diffState.files);

  // Initialize with all files selected
  $effect(() => {
    const paths = new Set<string>();
    for (const file of files) {
      const path = getFilePath(file);
      if (path) paths.add(path);
    }
    selectedPaths = paths;
  });

  function toggleFile(path: string) {
    const newSet = new Set(selectedPaths);
    if (newSet.has(path)) {
      newSet.delete(path);
    } else {
      newSet.add(path);
    }
    selectedPaths = newSet;
  }

  function toggleAll() {
    if (selectedPaths.size === files.length) {
      selectedPaths = new Set();
    } else {
      const paths = new Set<string>();
      for (const file of files) {
        const path = getFilePath(file);
        if (path) paths.add(path);
      }
      selectedPaths = paths;
    }
  }

  function getFilePath(file: FileDiffSummary): string {
    return file.after ?? file.before ?? '';
  }

  function getFileStatus(file: FileDiffSummary): 'added' | 'deleted' | 'modified' {
    if (!file.before) return 'added';
    if (!file.after) return 'deleted';
    return 'modified';
  }

  async function handleSubmit() {
    error = null;

    if (selectedPaths.size === 0) {
      error = 'Select at least one file to commit';
      return;
    }

    if (message.trim() === '') {
      error = 'Enter a commit message';
      return;
    }

    committing = true;

    try {
      await commit(Array.from(selectedPaths), message.trim(), repoPath ?? undefined);
      onCommit();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      committing = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    } else if (event.key === 'Enter' && event.metaKey) {
      handleSubmit();
      event.preventDefault();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <header class="modal-header">
      <h2>
        <GitCommitHorizontal size={16} />
        Git Commit
      </h2>
      <button class="close-btn" onclick={onClose}>
        <X size={16} />
      </button>
    </header>

    <div class="modal-body">
      <div class="message-section">
        <textarea id="commit-message" bind:value={message} placeholder="message..." rows="3"
        ></textarea>
      </div>

      <div class="files-section">
        <div class="files-header">
          <button class="toggle-all" onclick={toggleAll}>
            <span class="checkbox" class:checked={selectedPaths.size === files.length}>
              {#if selectedPaths.size === files.length}
                <Check size={10} />
              {/if}
            </span>
            <span class="label">
              {selectedPaths.size} of {files.length} files
            </span>
          </button>
        </div>
        <div class="files-list">
          {#each files as file}
            {@const path = getFilePath(file)}
            {@const status = getFileStatus(file)}
            <button class="file-item" onclick={() => toggleFile(path)}>
              <span class="checkbox" class:checked={selectedPaths.has(path)}>
                {#if selectedPaths.has(path)}
                  <Check size={10} />
                {/if}
              </span>
              <span class="file-path">{path}</span>
              <span
                class="file-status"
                class:added={status === 'added'}
                class:deleted={status === 'deleted'}
              >
                {status === 'added' ? '+' : status === 'deleted' ? '−' : '•'}
              </span>
            </button>
          {/each}
        </div>
      </div>

      {#if error}
        <div class="error">
          <AlertCircle size={14} />
          <span>{error}</span>
        </div>
      {/if}
    </div>

    <footer class="modal-footer">
      <span class="hint">⌘ Enter to commit</span>
      <div class="buttons">
        <button class="btn btn-secondary" onclick={onClose}>Cancel</button>
        <button
          class="btn btn-primary"
          onclick={handleSubmit}
          disabled={committing || selectedPaths.size === 0 || message.trim() === ''}
        >
          {committing ? 'Committing...' : 'Commit'}
        </button>
      </div>
    </footer>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 480px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .modal-header h2 :global(svg) {
    color: var(--text-muted);
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .modal-body {
    padding: 20px;
    overflow-y: auto;
    flex: 1;
  }

  .message-section {
    margin-bottom: 20px;
  }

  textarea {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    resize: vertical;
    box-sizing: border-box;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  textarea::placeholder {
    color: var(--text-faint);
  }

  textarea:focus {
    outline: none;
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .files-section {
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    overflow: hidden;
  }

  .files-header {
    padding: 8px 12px;
    background: var(--bg-primary);
    border-bottom: 1px solid var(--border-subtle);
  }

  .toggle-all {
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    padding: 0;
  }

  .toggle-all:hover {
    color: var(--text-primary);
  }

  .files-list {
    max-height: 200px;
    overflow-y: auto;
  }

  .file-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-primary);
    font-size: var(--size-xs);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .file-item:last-child {
    border-bottom: none;
  }

  .file-item:hover {
    background-color: var(--bg-hover);
  }

  .checkbox {
    width: 14px;
    height: 14px;
    border: 1px solid var(--border-muted);
    border-radius: 3px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    transition:
      background-color 0.1s,
      border-color 0.1s;
  }

  .checkbox.checked {
    background-color: var(--ui-accent);
    border-color: var(--ui-accent);
    color: var(--bg-primary);
  }

  .file-path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  .file-status {
    font-weight: 600;
    color: var(--text-muted);
  }

  .file-status.added {
    color: var(--status-added);
  }

  .file-status.deleted {
    color: var(--status-deleted);
  }

  .error {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    margin-top: 16px;
    background-color: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-top: 1px solid var(--border-subtle);
  }

  .hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .buttons {
    display: flex;
    gap: 8px;
  }

  .btn {
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.1s,
      opacity 0.1s;
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-secondary {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-secondary:hover:not(:disabled) {
    background: var(--border-subtle);
  }

  .btn-primary {
    background: var(--ui-accent);
    color: var(--bg-primary);
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }
</style>
