<!--
  NoteModal.svelte — Markdown viewer for a branch note

  Displays rendered markdown content in a clean modal.
  Read-only view.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import MessageCircle from '@lucide/svelte/icons/message-circle';
  import FileText from '@lucide/svelte/icons/file-text';
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import {
    countAssistantMessagesAfter,
    getBranchNoteBySession,
    getProjectNoteBySession,
    handleExternalLinkClick,
  } from '../../api/commands';
  import { formatChatButtonLabel, type LinkedNoteContext } from '../sessions/noteFreshness';
  import SessionChatPane from '../sessions/SessionChatPane.svelte';
  import type { DisplayRootInput } from '../sessions/pathDisplayRoots';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';
  import { registerSearchShortcutTarget } from '../keyboard/searchTargets';
  import { viewport } from '../../shared/viewport.svelte';
  import '../../shared/markdown/diagramStyles.css';
  import { extractMarkdownDiagramFences } from '../../shared/markdown/diagramFormats';
  import DiagramViewerModal from '../../shared/markdown/DiagramViewerModal.svelte';
  import {
    getMarkdownDiagramSvgMarkup,
    isMarkdownDiagramActivationKey,
  } from '../../shared/markdown/diagramViewer';
  import { loadPikchrRenderer, type PikchrRenderer } from '../../shared/markdown/pikchrRendering';
  import { noteMarkdownWithTitle, renderNoteMarkdown } from './noteMarkdown';
  import type { HashtagItem, ProjectRepo, Session } from '../../types';
  import { findHashtagItemForReference, renderHashtagTokens } from '../sessions/hashtagItems';
  import ReferenceNavControls from '../references/ReferenceNavControls.svelte';
  import type { HashtagClickInfo, ReferenceNavState } from '../references/referenceHistory.svelte';

  interface Props {
    open: boolean;
    title: string;
    content: string;
    onClose: () => void;
    /** When set, shows a button to open the associated chat session. */
    sessionId?: string | null;
    noteUpdatedAt?: number | null;
    onOpenSession?: (sessionId: string) => void;
    noteId?: string | null;
    noteKind?: 'branch' | 'project';
    branchId?: string | null;
    projectId?: string | null;
    repoDir?: DisplayRootInput;
    repoLabel?: Pick<ProjectRepo, 'githubRepo' | 'subpath' | 'headRepo'> | null;
    chatOpen?: boolean;
    onChatOpenChange?: (open: boolean) => void;
    /** Suggested next steps to show as action buttons at the bottom. */
    nextSteps?: { commitStep: string | null; noteStep: string | null } | null;
    /** Called when the user clicks a next-step button. */
    onStartSession?: (mode: 'commit' | 'note', prefill: string) => void;
    hashtagItems?: HashtagItem[];
    referenceNav?: ReferenceNavState;
    onHashtagClick?: (click: HashtagClickInfo) => void;
  }

  let {
    open,
    title,
    content,
    onClose,
    sessionId,
    noteUpdatedAt,
    noteId,
    noteKind = 'branch',
    branchId,
    projectId,
    repoDir,
    repoLabel = null,
    chatOpen = $bindable(false),
    onChatOpenChange,
    nextSteps,
    onStartSession,
    hashtagItems = [],
    referenceNav,
    onHashtagClick,
  }: Props = $props();

  let copied = $state(false);
  let liveNote = $state<{ id: string; title: string; content: string; updatedAt: number } | null>(
    null
  );
  let assistantMessagesAfterNote = $state(0);
  let assistantMessagesAfterNoteLoadedKey = $state<string | null>(null);
  let chatAutoOpenDecisionKey = $state<string | null>(null);
  let chatButtonLabel = $derived(formatChatButtonLabel(assistantMessagesAfterNote));
  let canOpenSession = $derived(Boolean(sessionId));
  let showChatInfo = $derived(canOpenSession && assistantMessagesAfterNote > 0 && !chatOpen);
  let displayTitle = $derived(liveNote?.title ?? title);
  let displayContent = $derived(liveNote?.content ?? content);
  let displayUpdatedAt = $derived(liveNote?.updatedAt ?? noteUpdatedAt);
  let displayNoteId = $derived(liveNote?.id ?? noteId);
  let hasNoteContent = $derived(!!displayContent.trim());
  let noteFreshnessKey = $derived(
    open && sessionId && typeof displayUpdatedAt === 'number'
      ? `${displayNoteId ?? 'note'}:${sessionId}:${displayUpdatedAt}`
      : null
  );
  let noteMarkdown = $derived(noteMarkdownWithTitle(displayTitle, displayContent));
  let splitChatOpen = $derived(chatOpen && viewport.canSplit && hasNoteContent);
  let chatOnly = $derived(chatOpen && (!viewport.canSplit || !hasNoteContent));
  let noteSearchAvailable = $derived(hasNoteContent && !chatOnly);
  let chatToggleLabel = $derived(
    chatOpen
      ? viewport.canSplit
        ? 'Hide chat'
        : 'View note'
      : viewport.canSplit
        ? 'Show chat'
        : 'View chat'
  );
  let chatToggleAriaLabel = $derived(
    chatOpen
      ? viewport.canSplit
        ? 'Hide chat pane'
        : 'View note pane'
      : viewport.canSplit
        ? 'Show chat pane'
        : 'View chat pane'
  );
  let contentClass = $derived(
    `h-[80vh] max-h-[900px] p-0 gap-0 overflow-hidden flex flex-col transition-[max-width] duration-150 ${splitChatOpen ? 'sm:max-w-[1080px]' : 'sm:max-w-[700px]'}`
  );
  let noteInfo = $derived<LinkedNoteContext | null>(
    displayNoteId
      ? {
          id: displayNoteId,
          title: displayTitle,
          content: displayContent,
          updatedAt: displayUpdatedAt ?? 0,
          hasParsedNote: hasNoteContent,
        }
      : null
  );
  let previousSessionStatus = $state<Session['status'] | null>(null);
  let noteHasPikchr = $derived(
    extractMarkdownDiagramFences(noteMarkdown).some((diagram) => diagram.language === 'pikchr')
  );
  let pikchrRenderer = $state<PikchrRenderer | null>(null);
  let pikchrRendererLoadKey = $derived(noteMarkdown);
  let pikchrRendererLoadFailedKey = $state<string | null>(null);
  let pikchrRendererLoadFailed = $derived(pikchrRendererLoadFailedKey === pikchrRendererLoadKey);
  let renderedNoteHtml = $derived(
    renderNoteMarkdown(noteMarkdown, {
      pikchrRenderer,
      renderInlineText: hashtagItems.length
        ? (text) => renderHashtagTokens(text, hashtagItems)
        : undefined,
    })
  );
  let diagramViewerSvg = $state<string | null>(null);

  // Search state
  let searchVisible = $state(false);
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];
  let contentEl: HTMLDivElement | undefined;
  let unregisterSearchTarget: (() => void) | null = null;

  // Register the global search-shortcut target only while the visible note pane can be searched.
  // Branch and project views lazy-mount this component, but this guard keeps
  // persistent callers from letting a closed note modal capture Cmd/Ctrl+F.
  $effect(() => {
    if (!open || !noteSearchAvailable) return;
    const unregister = registerSearchShortcutTarget({
      find: openSearch,
      next: nextMatch,
      previous: previousMatch,
    });
    unregisterSearchTarget = unregister;
    return () => {
      unregister();
      unregisterSearchTarget = null;
    };
  });

  $effect(() => {
    if (noteSearchAvailable || !searchVisible) return;
    closeSearch();
  });

  $effect(() => {
    if (!open || !noteHasPikchr || pikchrRenderer || pikchrRendererLoadFailed) return;

    const loadKey = pikchrRendererLoadKey;
    let stale = false;
    loadPikchrRenderer()
      .then((renderer) => {
        if (!stale && pikchrRendererLoadKey === loadKey) pikchrRenderer = renderer;
      })
      .catch(() => {
        if (!stale && pikchrRendererLoadKey === loadKey) pikchrRendererLoadFailedKey = loadKey;
      });

    return () => {
      stale = true;
    };
  });

  $effect(() => {
    if (!open) {
      pikchrRendererLoadFailedKey = null;
      chatAutoOpenDecisionKey = null;
      previousSessionStatus = null;
    }
  });

  $effect(() => {
    noteId;
    title;
    content;
    noteUpdatedAt;
    liveNote = null;
  });

  onDestroy(() => {
    unregisterSearchTarget?.();
  });

  $effect(() => {
    const key = noteFreshnessKey;
    const sid = sessionId;
    const updatedAt = displayUpdatedAt;
    if (!open || !key || !sid || typeof updatedAt !== 'number') {
      assistantMessagesAfterNote = 0;
      assistantMessagesAfterNoteLoadedKey = null;
      return;
    }

    assistantMessagesAfterNoteLoadedKey = null;
    let stale = false;
    countAssistantMessagesAfter(sid, updatedAt)
      .then((count) => {
        if (!stale) {
          assistantMessagesAfterNote = count;
          assistantMessagesAfterNoteLoadedKey = key;
        }
      })
      .catch(() => {
        if (!stale) {
          assistantMessagesAfterNote = 0;
          assistantMessagesAfterNoteLoadedKey = key;
        }
      });

    return () => {
      stale = true;
    };
  });

  $effect(() => {
    const key = noteFreshnessKey;
    if (!key || assistantMessagesAfterNoteLoadedKey !== key || chatAutoOpenDecisionKey === key) {
      return;
    }

    chatAutoOpenDecisionKey = key;
    if (assistantMessagesAfterNote > 0 && !chatOpen) {
      setChatOpen(true);
    }
  });

  function setChatOpen(next: boolean) {
    chatOpen = next;
    onChatOpenChange?.(next);
    if (!next) {
      previousSessionStatus = null;
    }
  }

  async function refreshLiveNote() {
    if (!sessionId) return;
    try {
      const refreshed =
        noteKind === 'project'
          ? await getProjectNoteBySession(sessionId)
          : await getBranchNoteBySession(sessionId);
      if (!refreshed) return;
      liveNote = {
        id: refreshed.id,
        title: refreshed.title,
        content: refreshed.content,
        updatedAt: refreshed.updatedAt,
      };
    } catch {
      // The next open or poll-backed refresh will try again.
    }
  }

  function handlePaneSessionChange(next: Session | null) {
    const wasActive = previousSessionStatus === 'running' || previousSessionStatus === 'queued';
    const isInactive = next && next.status !== 'running' && next.status !== 'queued';
    if ((wasActive && isInactive) || (!previousSessionStatus && isInactive && !hasNoteContent)) {
      void refreshLiveNote();
    }
    previousSessionStatus = next?.status ?? null;
  }

  function handleChatToggle() {
    if (!canOpenSession) return;
    if (chatOpen && !viewport.canSplit) {
      setChatOpen(false);
      return;
    }
    setChatOpen(!chatOpen);
  }

  async function handleShare() {
    try {
      await navigator.clipboard.writeText(noteMarkdown);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // clipboard API may fail in some contexts
    }
  }

  function openSearch() {
    if (!noteSearchAvailable) return;
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

  function openDiagramViewerFromEvent(event: MouseEvent | KeyboardEvent): boolean {
    const svgMarkup = getMarkdownDiagramSvgMarkup(event.target);
    if (!svgMarkup) return false;

    event.preventDefault();
    event.stopPropagation();
    diagramViewerSvg = svgMarkup;
    return true;
  }

  function hashtagClickFromTarget(target: EventTarget | null): HashtagClickInfo | null {
    if (!(target instanceof HTMLElement)) return null;
    const badge = target.closest<HTMLElement>('[data-hashtag-ref]');
    if (!badge) return null;
    const type = badge.dataset.hashtagType as HashtagItem['type'] | undefined;
    const id = badge.dataset.hashtagId;
    const ref = badge.dataset.hashtagRef;
    if (!type || !id || !ref) return null;
    const item = findHashtagItemForReference(hashtagItems, type, id);
    return { type, id, ref, item };
  }

  function handleContentClick(event: MouseEvent) {
    if (openDiagramViewerFromEvent(event)) return;

    const click = hashtagClickFromTarget(event.target);
    if (click && onHashtagClick) {
      event.preventDefault();
      event.stopPropagation();
      onHashtagClick(click);
      return;
    }
    handleExternalLinkClick(event);
  }

  function handleContentKeydown(event: KeyboardEvent) {
    if (isMarkdownDiagramActivationKey(event) && openDiagramViewerFromEvent(event)) return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const click = hashtagClickFromTarget(event.target);
    if (!click || !onHashtagClick) return;
    event.preventDefault();
    event.stopPropagation();
    onHashtagClick(click);
  }
</script>

{#snippet copyButton()}
  <Button
    variant="outline"
    size="sm"
    class={[
      'h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground',
      copied && 'text-[var(--status-added)] hover:text-[var(--status-added)]',
    ]}
    title={copied ? 'Copied!' : 'Copy note to clipboard'}
    aria-label={copied ? 'Copied!' : 'Copy note to clipboard'}
    onclick={handleShare}
  >
    {#if copied}
      <Check size={16} />
    {:else}
      <Copy size={16} />
    {/if}
  </Button>
{/snippet}

{#snippet chatToggleButton()}
  {#if canOpenSession}
    <Button
      variant="outline"
      size="sm"
      class="h-7 shrink-0 gap-1 border-[var(--border-muted)] bg-transparent px-2.5 text-xs text-muted-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
      title={chatToggleAriaLabel}
      aria-label={chatToggleAriaLabel}
      aria-pressed={chatOpen}
      onclick={handleChatToggle}
    >
      {#if chatOpen}
        <PanelRightClose size={15} aria-hidden="true" />
      {:else}
        <PanelRightOpen size={15} aria-hidden="true" />
      {/if}
      <span>{chatToggleLabel}</span>
    </Button>
  {/if}
{/snippet}

{#snippet closeButton()}
  <Button
    variant="ghost"
    size="icon-sm"
    class="size-7 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
    title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
    aria-label="Close"
    onclick={onClose}
  >
    <X size={16} />
  </Button>
{/snippet}

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
    class={contentClass}
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
  >
    <Dialog.Header class="gap-0 border-b border-[var(--border-subtle)] p-0 flex-shrink-0">
      <div class:split-chat-open={splitChatOpen} class="note-modal-header-grid">
        <div class="note-header-pane" class:split-pane={splitChatOpen}>
          {#if referenceNav}
            <ReferenceNavControls nav={referenceNav} />
          {/if}
          <div class="header-content">
            <span class="note-title-icon" aria-hidden="true">
              <FileText size={13} />
            </span>
            <Dialog.Title
              class="text-[var(--size-sm)] font-semibold text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
            >
              Note
            </Dialog.Title>
          </div>
          {#if noteSearchAvailable}
            <InContentSearch
              visible={searchVisible}
              {matchCount}
              currentIndex={currentMatchIndex}
              onSearch={performSearch}
              onNext={nextMatch}
              onPrevious={previousMatch}
              onClose={closeSearch}
            />
          {/if}
          <div class="note-header-actions">
            {#if hasNoteContent}
              {@render copyButton()}
            {/if}
            {#if !splitChatOpen && (hasNoteContent || !chatOpen)}
              {@render chatToggleButton()}
            {/if}
          </div>
        </div>
        {#if splitChatOpen}
          <div class="chat-header-pane">
            {@render chatToggleButton()}
            {@render closeButton()}
          </div>
        {:else}
          <div class="header-actions">
            {@render closeButton()}
          </div>
        {/if}
      </div>
    </Dialog.Header>
    <div class:split-chat-open={splitChatOpen} class:chat-only={chatOnly} class="modal-body">
      <section
        id="note-modal-note-pane"
        class="note-pane"
        aria-hidden={chatOnly}
        aria-label="Note content"
      >
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="modal-content"
          bind:this={contentEl}
          onclick={handleContentClick}
          onkeydown={handleContentKeydown}
        >
          {#if hasNoteContent}
            <div class="markdown-content">
              {@html renderedNoteHtml}
            </div>
          {:else}
            <p class="empty-note">This note has no content.</p>
          {/if}
        </div>

        {#if showChatInfo || (nextSteps && onStartSession && (nextSteps.noteStep || nextSteps.commitStep))}
          <div class="next-steps">
            {#if showChatInfo}
              <div class="chat-info-row">
                <Button
                  variant="outline"
                  class="h-auto min-h-9 max-w-full gap-2 border-[var(--border-muted)] bg-[var(--bg-chrome)] px-3.5 py-2 text-sm font-medium text-foreground shadow-none hover:border-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  onclick={() => setChatOpen(true)}
                >
                  <MessageCircle size={16} aria-hidden="true" />
                  <span class="min-w-0 truncate" title={chatButtonLabel}>{chatButtonLabel}</span>
                </Button>
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
      </section>

      {#if sessionId && chatOpen}
        <aside id="note-modal-chat-pane" class="chat-pane" aria-label="Chat">
          <SessionChatPane
            compact
            active={chatOpen}
            {sessionId}
            {repoDir}
            {branchId}
            {projectId}
            {repoLabel}
            {hashtagItems}
            {noteInfo}
            onSessionChange={handlePaneSessionChange}
            {onHashtagClick}
          />
        </aside>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<DiagramViewerModal
  open={diagramViewerSvg !== null}
  svgMarkup={diagramViewerSvg}
  onClose={() => (diagramViewerSvg = null)}
/>

<style>
  .note-modal-header-grid {
    display: flex;
    align-items: stretch;
    justify-content: space-between;
    width: 100%;
    min-width: 0;
  }

  .note-modal-header-grid.split-chat-open,
  .modal-body.split-chat-open {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(340px, 1fr);
  }

  .note-header-pane {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    flex: 1;
    padding: 12px 16px;
  }

  .note-header-pane.split-pane {
    flex: 2 1 0;
  }

  .chat-header-pane {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
    min-width: 340px;
    max-width: 390px;
    flex: 1 1 0;
    padding: 12px;
    border-left: 1px solid var(--border-subtle);
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .note-title-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    flex-shrink: 0;
    color: var(--note-color);
    background: var(--note-bg);
  }

  .note-header-actions,
  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .header-actions {
    padding: 12px 16px 12px 0;
  }

  .modal-body {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }

  .note-pane {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    flex-direction: column;
  }

  .split-chat-open .note-pane {
    flex: 2 1 0;
  }

  .chat-only .note-pane {
    display: none;
  }

  .chat-pane {
    display: flex;
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
    border-left: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .split-chat-open .chat-pane {
    min-width: 340px;
    max-width: 390px;
  }

  .chat-only .chat-pane {
    border-left: none;
    max-width: none;
  }

  .modal-content {
    flex: 1;
    overflow-y: auto;
    padding: 24px;
    min-height: 0;
    background: var(--bg-primary);
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

  .markdown-content :global(ul) {
    list-style-type: disc;
  }

  .markdown-content :global(ol) {
    list-style-type: decimal;
  }

  .markdown-content :global(ul ul) {
    list-style-type: circle;
  }

  .markdown-content :global(ul ul ul) {
    list-style-type: square;
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
    background: var(--bg-chrome);
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
