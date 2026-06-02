<!--
  NoteModal.svelte — Markdown viewer for a branch note

  Displays a note's title and rendered markdown content in a clean modal.
  Read-only view.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import MessageCircle from '@lucide/svelte/icons/message-circle';
  import { marked } from 'marked';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { sanitize } from '../../shared/sanitize';
  import { countAssistantMessagesAfter, handleExternalLinkClick } from '../../api/commands';
  import { formatChatButtonLabel } from '../sessions/noteFreshness';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';
  import { registerSearchShortcutTarget } from '../keyboard/searchTargets';
  import { viewport } from '../../shared/viewport.svelte';

  marked.setOptions({ breaks: true, gfm: true });

  interface Props {
    open: boolean;
    title: string;
    content: string;
    onClose: () => void;
    /** When set, shows a button to open the associated chat session. */
    sessionId?: string | null;
    noteUpdatedAt?: number | null;
    onOpenSession?: (sessionId: string) => void;
    /** Suggested next steps to show as action buttons at the bottom. */
    nextSteps?: { commitStep: string | null; noteStep: string | null } | null;
    /** Called when the user clicks a next-step button. */
    onStartSession?: (mode: 'commit' | 'note', prefill: string) => void;
  }

  let {
    open,
    title,
    content,
    onClose,
    sessionId,
    noteUpdatedAt,
    onOpenSession,
    nextSteps,
    onStartSession,
  }: Props = $props();

  let copied = $state(false);
  let assistantMessagesAfterNote = $state(0);
  let chatButtonLabel = $derived(formatChatButtonLabel(assistantMessagesAfterNote));
  let canOpenSession = $derived(Boolean(sessionId && onOpenSession));
  let showChatInfo = $derived(canOpenSession && assistantMessagesAfterNote > 0);

  // Search state
  let searchVisible = $state(false);
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];
  let contentEl: HTMLDivElement;
  let unregisterSearchTarget: (() => void) | null = null;

  onMount(() => {
    unregisterSearchTarget = registerSearchShortcutTarget({
      find: openSearch,
      next: nextMatch,
      previous: previousMatch,
    });
  });

  onDestroy(() => {
    unregisterSearchTarget?.();
  });

  $effect(() => {
    const sid = sessionId;
    const updatedAt = noteUpdatedAt;
    if (!sid || typeof updatedAt !== 'number') {
      assistantMessagesAfterNote = 0;
      return;
    }

    let stale = false;
    countAssistantMessagesAfter(sid, updatedAt)
      .then((count) => {
        if (!stale) {
          assistantMessagesAfterNote = count;
        }
      })
      .catch(() => {
        if (!stale) assistantMessagesAfterNote = 0;
      });

    return () => {
      stale = true;
    };
  });

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

    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    currentMatchIndex = (currentMatchIndex + 1) % matchElements.length;

    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex]);
  }

  function previousMatch() {
    if (matchElements.length === 0) return;

    if (matchElements[currentMatchIndex]) {
      matchElements[currentMatchIndex].classList.remove('search-match-current');
    }

    currentMatchIndex = (currentMatchIndex - 1 + matchElements.length) % matchElements.length;

    matchElements[currentMatchIndex].classList.add('search-match-current');
    scrollToMatch(matchElements[currentMatchIndex]);
  }
</script>

<Dialog.Root
  {open}
  onOpenChange={(v) => {
    if (!v) {
      if (searchVisible) {
        closeSearch();
      }
      onClose();
    }
  }}
>
  <Dialog.Content
    class="sm:max-w-[700px] h-[80vh] max-h-[900px] p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)] flex-shrink-0"
    >
      <div class="header-content">
        <Dialog.Title
          class="text-[var(--size-sm)] font-semibold text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
        >
          {title}
        </Dialog.Title>
      </div>
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
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="outline"
                size="sm"
                class={[
                  'h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground',
                  copied && 'text-[var(--status-added)] hover:text-[var(--status-added)]',
                ]}
                onclick={handleShare}
              >
                {#if copied}
                  <Check size={16} />
                {:else}
                  <Copy size={16} />
                {/if}
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>{copied ? 'Copied!' : 'Copy note to clipboard'}</Tooltip.Content>
        </Tooltip.Root>
        {#if canOpenSession}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="sm"
                  class="h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  onclick={() => onOpenSession?.(sessionId!)}
                >
                  View chat
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Open chat session</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="ghost"
                size="icon-sm"
                class="size-7 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
                onclick={onClose}
              >
                <X size={16} />
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>
            {viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          </Tooltip.Content>
        </Tooltip.Root>
      </div>
    </Dialog.Header>
    <div class="modal-body">
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
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
    {#if showChatInfo || (nextSteps && onStartSession && (nextSteps.noteStep || nextSteps.commitStep))}
      <div class="next-steps">
        {#if showChatInfo}
          <div class="chat-info-row">
            <Tooltip.Root>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <Button
                    {...props}
                    variant="outline"
                    class="h-auto min-h-9 max-w-full gap-2 border-[var(--border-muted)] bg-[var(--bg-chrome)] px-3.5 py-2 text-sm font-medium text-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                    onclick={() => onOpenSession?.(sessionId!)}
                  >
                    <MessageCircle size={16} aria-hidden="true" />
                    <span class="min-w-0 truncate">{chatButtonLabel}</span>
                  </Button>
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Content>Open chat session</Tooltip.Content>
            </Tooltip.Root>
          </div>
        {/if}
        {#if nextSteps && onStartSession && nextSteps.noteStep}
          <div class="next-step-row">
            <span class="next-step-prompt">{nextSteps.noteStep}</span>
            <Button
              variant="outline"
              size="sm"
              class="h-auto shrink-0 rounded-md border-transparent bg-[var(--note-bg)] px-3 py-1 text-xs font-medium text-[var(--note-color)] shadow-none hover:border-transparent hover:bg-[var(--note-bg-emphasis)] hover:text-[var(--note-color)]"
              onclick={() => onStartSession('note', nextSteps!.noteStep!)}
            >
              Start note
            </Button>
          </div>
        {/if}
        {#if nextSteps && onStartSession && nextSteps.commitStep}
          <div class="next-step-row">
            <span class="next-step-prompt">{nextSteps.commitStep}</span>
            <Button
              variant="outline"
              size="sm"
              class="h-auto shrink-0 rounded-md border-transparent bg-[var(--commit-bg)] px-3 py-1 text-xs font-medium text-[var(--commit-color)] shadow-none hover:border-transparent hover:bg-[var(--commit-bg-emphasis)] hover:text-[var(--commit-color)]"
              onclick={() => onStartSession('commit', nextSteps!.commitStep!)}
            >
              Start commit
            </Button>
          </div>
        {/if}
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .modal-body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
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

  .next-steps {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .next-step-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    background: var(--bg-elevated);
    border-radius: 8px;
  }

  .next-step-prompt {
    flex: 1;
    font-size: var(--size-sm);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .chat-info-row {
    display: flex;
    justify-content: center;
  }
</style>
