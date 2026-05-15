<!--
  InContentSearch.svelte — Reusable in-content search bar

  Provides a compact search UI with keyboard navigation support.
  Displays match counter and navigation buttons.
-->
<script lang="ts">
  import { ChevronUp, ChevronDown, X } from 'lucide-svelte';
  import { tick } from 'svelte';
  import { viewport } from './viewport.svelte';

  interface Props {
    visible: boolean;
    matchCount: number;
    currentIndex: number;
    onSearch: (query: string) => void;
    onNext: () => void;
    onPrevious: () => void;
    onClose: () => void;
  }

  let { visible, matchCount, currentIndex, onSearch, onNext, onPrevious, onClose }: Props =
    $props();

  let inputEl = $state<HTMLInputElement>();
  let query = $state('');

  // Auto-focus input when search becomes visible
  $effect(() => {
    if (visible) {
      tick().then(() => inputEl?.focus());
    }
  });

  // Notify parent when query changes
  $effect(() => {
    onSearch(query);
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (e.shiftKey) {
        onPrevious();
      } else {
        onNext();
      }
    }
  }

  // Display match counter (1-based for users)
  let counterText = $derived.by(() => {
    if (!query.trim()) return '';
    if (matchCount === 0) return 'No matches';
    return `${currentIndex + 1} of ${matchCount}`;
  });
</script>

{#if visible}
  <div class="search-bar">
    <input
      bind:this={inputEl}
      bind:value={query}
      type="text"
      class="search-input"
      placeholder="Search..."
      onkeydown={handleKeydown}
    />
    {#if counterText}
      <span class="match-counter">{counterText}</span>
    {/if}
    <button
      class="search-btn"
      onclick={onPrevious}
      disabled={matchCount === 0}
      title={viewport.hasKeyboard ? 'Previous match (Shift+Enter)' : 'Previous match'}
    >
      <ChevronUp size={14} />
    </button>
    <button
      class="search-btn"
      onclick={onNext}
      disabled={matchCount === 0}
      title={viewport.hasKeyboard ? 'Next match (Enter)' : 'Next match'}
    >
      <ChevronDown size={14} />
    </button>
    <button
      class="search-btn close-search"
      onclick={onClose}
      title={viewport.hasKeyboard ? 'Close search (Esc)' : 'Close search'}
    >
      <X size={14} />
    </button>
  </div>
{/if}

<style>
  .search-bar {
    position: absolute;
    top: 16px;
    right: 56px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    z-index: 10;
  }

  .search-input {
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-md);
    color: var(--text-primary);
    width: 200px;
    font-family: inherit;
  }

  .search-input::placeholder {
    color: var(--text-faint);
  }

  .match-counter {
    font-size: var(--size-xs);
    color: var(--text-muted);
    white-space: nowrap;
    min-width: 60px;
    text-align: center;
  }

  .search-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 4px;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .search-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .search-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .close-search {
    margin-left: 4px;
  }
</style>
