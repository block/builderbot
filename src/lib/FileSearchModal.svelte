<!--
  FileSearchModal.svelte - Fuzzy file search for adding reference files

  Opens with Cmd+O to search for files in the repository.
  Selected files are added as reference files for viewing/commenting.
-->
<script lang="ts">
  import { Search, File, X, Loader2 } from 'lucide-svelte';
  import { searchFiles } from './services/files';

  interface Props {
    /** Git ref to search files at (e.g., HEAD, branch name) */
    refName: string;
    /** Repository path */
    repoPath?: string;
    /** Called when a file is selected */
    onSelect: (path: string) => void;
    /** Called when modal is closed */
    onClose: () => void;
    /** Paths that are already added (to show as disabled) */
    existingPaths?: string[];
  }

  let { refName, repoPath, onSelect, onClose, existingPaths = [] }: Props = $props();

  let query = $state('');
  let results = $state<string[]>([]);
  let selectedIndex = $state(0);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let searchTimeout: ReturnType<typeof setTimeout> | null = null;
  let inputEl: HTMLInputElement | null = $state(null);

  // Focus input on mount
  $effect(() => {
    if (inputEl) {
      inputEl.focus();
    }
  });

  // Debounced search
  $effect(() => {
    const q = query;

    if (searchTimeout) {
      clearTimeout(searchTimeout);
    }

    if (q.length === 0) {
      results = [];
      loading = false;
      return;
    }

    loading = true;
    error = null;

    searchTimeout = setTimeout(async () => {
      try {
        results = await searchFiles(refName, q, 20, repoPath);
        selectedIndex = 0;
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
        results = [];
      } finally {
        loading = false;
      }
    }, 150);
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    } else if (event.key === 'Enter' && results.length > 0) {
      const selected = results[selectedIndex];
      if (selected && !existingPaths.includes(selected)) {
        onSelect(selected);
      }
      event.preventDefault();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, results.length - 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }

  function selectResult(path: string) {
    if (!existingPaths.includes(path)) {
      onSelect(path);
    }
  }

  /**
   * Highlight matching parts of a path.
   * Returns HTML with <mark> tags around matching characters.
   */
  function highlightMatch(path: string, q: string): string {
    if (!q) return escapeHtml(path);

    const pathLower = path.toLowerCase();
    const queryLower = q.toLowerCase();
    let result = '';
    let queryIdx = 0;

    for (let i = 0; i < path.length; i++) {
      if (queryIdx < queryLower.length && pathLower[i] === queryLower[queryIdx]) {
        result += `<mark>${escapeHtml(path[i])}</mark>`;
        queryIdx++;
      } else {
        result += escapeHtml(path[i]);
      }
    }

    return result;
  }

  function escapeHtml(str: string): string {
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
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
    <div class="search-container">
      <div class="search-icon">
        {#if loading}
          <Loader2 size={18} class="spinner" />
        {:else}
          <Search size={18} />
        {/if}
      </div>
      <input
        bind:this={inputEl}
        type="text"
        class="search-input"
        placeholder="Search files..."
        bind:value={query}
        autocomplete="off"
        spellcheck="false"
      />
      <button class="close-btn" onclick={onClose} title="Close (Esc)">
        <X size={18} />
      </button>
    </div>

    {#if error}
      <div class="error">{error}</div>
    {/if}

    <div class="results">
      {#if results.length === 0 && query.length > 0 && !loading}
        <div class="empty-state">No files found</div>
      {:else}
        {#each results as path, i (path)}
          {@const isExisting = existingPaths.includes(path)}
          <button
            class="result"
            class:selected={i === selectedIndex}
            class:existing={isExisting}
            onclick={() => selectResult(path)}
            disabled={isExisting}
            onmouseenter={() => (selectedIndex = i)}
          >
            <File size={14} />
            <span class="path">{@html highlightMatch(path, query)}</span>
            {#if isExisting}
              <span class="badge">Added</span>
            {/if}
          </button>
        {/each}
      {/if}
    </div>

    <div class="footer">
      <span class="hint">
        <kbd>↑↓</kbd> navigate
        <kbd>Enter</kbd> select
        <kbd>Esc</kbd> close
      </span>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 15vh;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 560px;
    max-width: 90vw;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .search-container {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .search-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  :global(.spinner) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .search-input {
    flex: 1;
    background: none;
    border: none;
    font-size: var(--size-base);
    color: var(--text-primary);
    outline: none;
  }

  .search-input::placeholder {
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

  .error {
    padding: 12px 16px;
    font-size: var(--size-sm);
    color: var(--status-deleted);
    background: var(--bg-secondary);
  }

  .results {
    flex: 1;
    overflow-y: auto;
    padding: 8px 0;
  }

  .empty-state {
    padding: 24px 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .result {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 16px;
    background: none;
    border: none;
    text-align: left;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .result:hover,
  .result.selected {
    background-color: var(--bg-hover);
  }

  .result.existing {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .result :global(svg) {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-sm) - 1px);
  }

  .path :global(mark) {
    background: var(--bg-primary);
    color: var(--text-accent);
    border-radius: 2px;
    padding: 0 1px;
  }

  .badge {
    font-size: calc(var(--size-xs) - 1px);
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--bg-secondary);
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .footer {
    padding: 10px 16px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-secondary);
  }

  .hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  kbd {
    display: inline-block;
    padding: 2px 5px;
    margin: 0 4px;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 3px;
  }
</style>
