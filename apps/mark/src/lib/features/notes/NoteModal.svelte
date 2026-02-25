<!--
  NoteModal.svelte — Markdown viewer for a branch note

  Displays a note's title and rendered markdown content in a clean modal.
  Read-only view.
-->
<script lang="ts">
  import { X, Copy, Check } from 'lucide-svelte';
  import { marked } from 'marked';
  import { sanitize } from '../../shared/sanitize';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import { handleExternalLinkClick } from '../../api/commands';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';

  marked.setOptions({ breaks: true, gfm: true });

  interface Props {
    title: string;
    content: string;
    onClose: () => void;
  }

  let { title, content, onClose }: Props = $props();

  let copied = $state(false);
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: () => onClose() });

  // Search state
  let searchVisible = $state(false);
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];
  let contentEl: HTMLDivElement;

  function renderMarkdown(text: string): string {
    return sanitize(marked.parse(text) as string);
  }

  async function handleShare() {
    const text = title ? `# ${title}\n\n${content}` : content;
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // clipboard API may fail in some contexts
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    const isMac = navigator.platform.toLowerCase().includes('mac');
    const cmdKey = isMac ? e.metaKey : e.ctrlKey;

    // Handle Escape key
    if (e.key === 'Escape') {
      e.preventDefault();
      if (searchVisible) {
        closeSearch();
      } else {
        onClose();
      }
      return;
    }

    // Handle search shortcuts
    if (cmdKey && e.key === 'f') {
      e.preventDefault();
      openSearch();
    } else if (cmdKey && e.key === 'g') {
      e.preventDefault();
      if (e.shiftKey) {
        previousMatch();
      } else {
        nextMatch();
      }
    }
  }

  function openSearch() {
    searchVisible = true;
  }

  function closeSearch() {
    searchVisible = false;
    searchQuery = '';
    if (contentEl) {
      clearHighlights(contentEl);
    }
    matchCount = 0;
    currentMatchIndex = 0;
    matchElements = [];
  }

  function performSearch(query: string) {
    if (!contentEl) return;

    // Clear previous highlights
    clearHighlights(contentEl);
    matchElements = [];
    matchCount = 0;
    currentMatchIndex = 0;

    // If query is empty, nothing to highlight
    if (!query.trim()) return;

    // Highlight matches
    const result = highlightMatches(contentEl, query, currentMatchIndex);
    matchElements = result.elements;
    matchCount = result.total;

    // Scroll to first match if any
    if (matchElements.length > 0) {
      scrollToMatch(matchElements[0]);
    }
  }

  function nextMatch() {
    if (matchElements.length === 0) return;

    // Remove current class from old match
    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    // Cycle to next match (wrap around)
    currentMatchIndex = (currentMatchIndex + 1) % matchElements.length;

    // Add current class to new match
    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex]);
  }

  function previousMatch() {
    if (matchElements.length === 0) return;

    // Remove current class from old match
    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    // Cycle to previous match (wrap around)
    currentMatchIndex = (currentMatchIndex - 1 + matchElements.length) % matchElements.length;

    // Add current class to new match
    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex]);
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()}>
    <header class="modal-header">
      <h2 class="modal-title">{title}</h2>
      <InContentSearch
        visible={searchVisible}
        {matchCount}
        currentIndex={currentMatchIndex}
        onSearch={performSearch}
        onNext={nextMatch}
        onPrevious={previousMatch}
        onClose={closeSearch}
      />
      <div class="header-actions">
        <button
          class="share-btn"
          class:copied
          onclick={handleShare}
          title={copied ? 'Copied!' : 'Copy note to clipboard'}
        >
          {#if copied}
            <Check size={16} />
          {:else}
            <Copy size={16} />
          {/if}
        </button>
        <button class="close-btn" onclick={onClose} title="Close (Esc)">
          <X size={16} />
        </button>
      </div>
    </header>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="modal-content" bind:this={contentEl} onclick={handleExternalLinkClick}>
      {#if content.trim()}
        <div class="markdown-content">
          {@html renderMarkdown(content)}
        </div>
      {:else}
        <p class="empty-note">This note has no content.</p>
      {/if}
    </div>
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
    display: flex;
    flex-direction: column;
    width: 640px;
    max-width: 90vw;
    max-height: 80vh;
    background: var(--bg-chrome);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: var(--shadow-elevated);
  }

  .modal-header {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
    gap: 12px;
  }

  .modal-title {
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .share-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .share-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .share-btn.copied {
    color: var(--status-added);
  }

  .modal-content {
    flex: 1;
    overflow-y: auto;
    padding: 24px;
    min-height: 0;
  }

  .modal-content::-webkit-scrollbar {
    width: 6px;
  }

  .modal-content::-webkit-scrollbar-track {
    background: transparent;
  }

  .modal-content::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 3px;
  }

  .modal-content::-webkit-scrollbar-thumb:hover {
    background: var(--scrollbar-thumb-hover-transparent);
  }

  .empty-note {
    margin: 0;
    color: var(--text-faint);
    font-size: var(--size-sm);
    font-style: italic;
    text-align: center;
    padding: 24px 0;
  }

  /* Markdown styles — matches SessionModal */
  .markdown-content {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.7;
  }

  .markdown-content :global(p) {
    margin: 0 0 0.75em 0;
  }

  .markdown-content :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3),
  .markdown-content :global(h4) {
    margin: 1em 0 0.5em 0;
    font-weight: 600;
    line-height: 1.3;
  }

  .markdown-content :global(h1:first-child),
  .markdown-content :global(h2:first-child),
  .markdown-content :global(h3:first-child),
  .markdown-content :global(h4:first-child) {
    margin-top: 0;
  }

  .markdown-content :global(h1) {
    font-size: 1.25em;
  }
  .markdown-content :global(h2) {
    font-size: 1.15em;
  }
  .markdown-content :global(h3) {
    font-size: 1.05em;
  }

  .markdown-content :global(ul),
  .markdown-content :global(ol) {
    margin: 0.5em 0;
    padding-left: 1.5em;
  }

  .markdown-content :global(li) {
    margin: 0.25em 0;
  }

  .markdown-content :global(code) {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: 0.9em;
    background: var(--bg-elevated, var(--bg-hover));
    padding: 0.15em 0.35em;
    border-radius: 3px;
  }

  .markdown-content :global(pre) {
    margin: 0.75em 0;
    padding: 0.75em;
    background: var(--bg-elevated, var(--bg-hover));
    border-radius: 6px;
    overflow-x: auto;
  }

  .markdown-content :global(pre code) {
    background: none;
    padding: 0;
    font-size: 0.85em;
  }

  .markdown-content :global(blockquote) {
    margin: 0.5em 0;
    padding-left: 0.75em;
    border-left: 3px solid var(--border-muted);
    color: var(--text-muted);
  }

  .markdown-content :global(a) {
    color: var(--ui-accent);
    text-decoration: none;
  }

  .markdown-content :global(a:hover) {
    text-decoration: underline;
  }

  .markdown-content :global(strong) {
    font-weight: 600;
  }

  .markdown-content :global(hr) {
    margin: 1em 0;
    border: none;
    border-top: 1px solid var(--border-subtle);
  }

  .markdown-content :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 0.75em 0;
  }

  .markdown-content :global(th),
  .markdown-content :global(td) {
    border: 1px solid var(--border-subtle);
    padding: 6px 12px;
    text-align: left;
  }

  .markdown-content :global(th) {
    background: var(--bg-primary);
    font-weight: 600;
  }
</style>
