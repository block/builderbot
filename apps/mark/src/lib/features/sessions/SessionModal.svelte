<!--
  SessionModal.svelte — View an AI session (live or historical)

  The single session viewer used everywhere in the app. Shows the message
  transcript with user messages as right-aligned chat bubbles, assistant
  messages as plain markdown, and tool calls as collapsible cards with
  argument previews.

  Features:
  - Text input always available at the bottom
  - Send button when idle, Stop button when running
  - Message queue: hitting Enter while running enqueues for after completion
  - Copy button on every message
  - Tool calls show name + args preview; expand to see output
  - Fixed-size modal with proper scrolling

  For running sessions, polls the DB incrementally:
  - Re-fetches the last known message (it may have grown from streaming)
  - Fetches any new messages after it
  - Stops polling when status leaves "running"

  Props:
    sessionId — the session to display
    onClose   — callback to close this modal
-->
<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import {
    X,
    AlertCircle,
    CircleStop,
    Send,
    Copy,
    Check,
    ChevronRight,
    ChevronDown,
    Zap,
    GitBranch,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { marked } from 'marked';
  import { sanitize } from '../../shared/sanitize';
  import type { Session, SessionMessage } from '../../types';
  import {
    cancelSession,
    getSession,
    getSessionMessages,
    getSessionMessagesSince,
    handleExternalLinkClick,
    resumeSession,
  } from '../../commands';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import { formatToolArgs, formatToolName, hasXmlBlocks } from './sessionModalHelpers';
  import InContentSearch from '../../shared/InContentSearch.svelte';
  import { highlightMatches, clearHighlights, scrollToMatch } from '../../shared/textHighlight';

  // Configure marked
  marked.setOptions({ breaks: true, gfm: true });

  interface Props {
    sessionId: string;
    onClose: () => void;
  }

  let { sessionId, onClose }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  let session = $state<Session | null>(null);
  let messages = $state<SessionMessage[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let cancelling = $state(false);
  let messagesEl: HTMLDivElement;
  let inputEl: HTMLTextAreaElement;
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollInFlight = false;
  let closed = false;
  let dismissing = $state(false);

  let inputText = $state('');
  let messageQueue = $state<string[]>([]);
  let copiedId = $state<number | string | null>(null);
  let expandedTools = $state<Set<number>>(new Set());

  let isLive = $derived(session?.status === 'running');
  let hasQueuedMessages = $derived(messageQueue.length > 0);

  // Search state
  let searchVisible = $state(false);
  let searchQuery = $state('');
  let matchCount = $state(0);
  let currentMatchIndex = $state(0);
  let matchElements: HTMLElement[] = [];

  // =========================================================================
  // Lifecycle
  // =========================================================================

  onMount(async () => {
    await loadSession();
    if (session?.status === 'running') {
      startPolling();
    }
    // Focus input on open
    tick().then(() => inputEl?.focus());
  });

  onDestroy(() => {
    closed = true;
    stopPolling();
  });

  // =========================================================================
  // Data loading
  // =========================================================================

  /** Initial full load. */
  async function loadSession() {
    if (closed) return;
    loading = true;
    error = null;
    try {
      const [s, msgs] = await Promise.all([getSession(sessionId), getSessionMessages(sessionId)]);
      if (closed) return;
      if (!s) {
        error = 'Session not found';
        return;
      }
      session = s;
      messages = msgs;
      scrollToBottom();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  /** Incremental poll — re-fetch last message (may have grown) + any new ones. */
  async function poll() {
    if (!session || closed || pollInFlight) return;
    pollInFlight = true;
    try {
      // Fetch session status
      const s = await getSession(sessionId);
      if (closed) return;
      if (s) session = s;

      // Incremental message fetch
      if (messages.length === 0) {
        const msgs = await getSessionMessages(sessionId);
        if (closed) return;
        if (msgs.length > 0) {
          messages = msgs;
          scrollToBottomIfNear();
        }
      } else {
        const lastId = messages[messages.length - 1].id;
        const updated = await getSessionMessagesSince(sessionId, lastId);
        if (closed) return;
        if (updated.length > 0) {
          const prev = messages.slice(0, -1);
          messages = [...prev, ...updated];
          if (updated.length > 1 || updated[0].id !== lastId) {
            scrollToBottomIfNear();
          }
        }
      }

      // Stop polling when session is done; process queued messages
      if (s && s.status !== 'running') {
        stopPolling();
        processQueue();
      }
    } catch {
      // Polling errors are expected during shutdown — silently ignore
    } finally {
      pollInFlight = false;
    }
  }

  function startPolling() {
    if (pollTimer) return;
    pollTimer = setInterval(poll, 500);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    pollInFlight = false;
  }

  // =========================================================================
  // Message sending & queue
  // =========================================================================

  let sending = $state(false);

  async function handleSend() {
    const text = inputText.trim();
    if (!text) return;
    inputText = '';
    // Reset textarea height after clearing (oninput won't fire for programmatic changes)
    tick().then(() => autoResize());

    if (isLive) {
      // Session is running — queue the message for after it finishes
      messageQueue = [...messageQueue, text];
      return;
    }

    // Session is idle — send immediately
    await sendMessage(text);
  }

  /** Actually send a message to the backend and start the agent. */
  async function sendMessage(text: string) {
    if (!session || sending) return;
    sending = true;
    error = null;
    try {
      await resumeSession(session.id, text);
      // Backend sets status to running and emits an event.
      // Force an immediate poll to pick up the new user message + status.
      session = { ...session, status: 'running' };
      startPolling();
      scrollToBottom();
    } catch (e) {
      error = `Failed to send: ${e instanceof Error ? e.message : String(e)}`;
      // Clear the queue — don't keep trying to send if the session is broken
      messageQueue = [];
    } finally {
      sending = false;
    }
  }

  /** Process the next queued message when the session becomes idle. */
  async function processQueue() {
    if (messageQueue.length === 0 || isLive || error) return;
    const [next, ...rest] = messageQueue;
    messageQueue = rest;
    await sendMessage(next);
  }

  async function handleCancel() {
    if (!session || cancelling) return;
    cancelling = true;
    try {
      await cancelSession(session.id);
    } catch (e) {
      error = `Failed to cancel: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      cancelling = false;
    }
  }

  // =========================================================================
  // Input handling
  // =========================================================================

  function handleInputKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  function autoResize() {
    if (!inputEl) return;
    inputEl.style.height = 'auto';
    inputEl.style.height = Math.min(inputEl.scrollHeight, 120) + 'px';
  }

  // =========================================================================
  // Copy
  // =========================================================================

  async function copyContent(content: string, id: number | string) {
    try {
      await navigator.clipboard.writeText(content);
      copiedId = id;
      setTimeout(() => {
        if (copiedId === id) copiedId = null;
      }, 1500);
    } catch {
      // clipboard API may fail in some contexts
    }
  }

  // =========================================================================
  // Tool call expand/collapse
  // =========================================================================

  function toggleTool(msgId: number) {
    const next = new Set(expandedTools);
    if (next.has(msgId)) {
      next.delete(msgId);
    } else {
      next.add(msgId);
    }
    expandedTools = next;
  }

  // =========================================================================
  // Helpers
  // =========================================================================

  /** Whether the user has intentionally scrolled up (disables auto-scroll). */
  let userScrolledUp = $state(false);
  let lastScrollTop = 0;

  function handleScroll() {
    if (!messagesEl) return;
    const { scrollTop, scrollHeight, clientHeight } = messagesEl;
    const atBottom = scrollHeight - scrollTop - clientHeight < 1;

    if (scrollTop < lastScrollTop && !atBottom) {
      // Scroll position moved upward — user scrolled up (any input method)
      userScrolledUp = true;
    }
    if (atBottom) {
      // They're back at bottom — re-enable auto-scroll
      userScrolledUp = false;
    }
    lastScrollTop = scrollTop;
  }

  /** Scroll to bottom unconditionally (e.g. initial load, user sends message). */
  function scrollToBottom() {
    tick().then(() => {
      if (messagesEl) {
        messagesEl.scrollTop = messagesEl.scrollHeight;
        userScrolledUp = false;
      }
    });
  }

  /** Scroll to bottom only if the user hasn't intentionally scrolled up. */
  function scrollToBottomIfNear() {
    if (!userScrolledUp) {
      scrollToBottom();
    }
  }

  function renderMarkdown(content: string): string {
    return sanitize(marked.parse(content) as string);
  }

  // =========================================================================
  // XML tag parsing (action / branch-history blocks)
  // =========================================================================

  type ContentSegment =
    | { type: 'text'; text: string }
    | { type: 'xml-block'; tag: string; label: string; content: string; icon: typeof Zap };

  /** Parse content into segments, extracting XML-style tagged blocks. */
  function parseContentSegments(content: string): ContentSegment[] {
    const segments: ContentSegment[] = [];
    let remaining = content;

    const tagPattern = /<(action|branch-history)>([\s\S]*?)<\/\1>/g;
    let lastIndex = 0;
    let match: RegExpExecArray | null;

    while ((match = tagPattern.exec(remaining)) !== null) {
      // Text before this tag
      if (match.index > lastIndex) {
        const text = remaining.slice(lastIndex, match.index).trim();
        if (text) segments.push({ type: 'text', text });
      }

      const tag = match[1];
      const label = tag === 'action' ? 'Action instructions' : 'Branch history';
      const icon = tag === 'action' ? Zap : GitBranch;
      segments.push({ type: 'xml-block', tag, label, content: match[2].trim(), icon });

      lastIndex = match.index + match[0].length;
    }

    // Remaining text after last tag
    if (lastIndex < remaining.length) {
      const text = remaining.slice(lastIndex).trim();
      if (text) segments.push({ type: 'text', text });
    }

    // If no tags found, return single text segment
    if (segments.length === 0 && content.trim()) {
      segments.push({ type: 'text', text: content });
    }

    return segments;
  }

  /** Track which XML blocks are expanded */
  let expandedXmlBlocks = $state<Set<string>>(new Set());
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: requestClose });

  function toggleXmlBlock(key: string) {
    const next = new Set(expandedXmlBlocks);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedXmlBlocks = next;
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
        requestClose();
      }
      return;
    }

    // Handle search shortcuts
    const target = e.target as HTMLElement;
    const isTypingInInput =
      target.tagName === 'TEXTAREA' && target.classList.contains('message-input');

    // Cmd+F should work even from the input field to open search
    if (cmdKey && e.key === 'f') {
      e.preventDefault();
      openSearch();
    }
    // Cmd+G navigation only works when not typing in input
    else if (!isTypingInInput && cmdKey && e.key === 'g') {
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
    if (messagesEl) {
      clearHighlights(messagesEl);
    }
    matchCount = 0;
    currentMatchIndex = 0;
    matchElements = [];
  }

  function performSearch(query: string) {
    if (!messagesEl) return;

    // Clear previous highlights
    clearHighlights(messagesEl);
    matchElements = [];
    matchCount = 0;
    currentMatchIndex = 0;

    // If query is empty, nothing to highlight
    if (!query.trim()) return;

    // Highlight matches
    const result = highlightMatches(messagesEl, query, currentMatchIndex);
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

  // Re-apply search when messages change (for live sessions)
  $effect(() => {
    if (searchVisible && searchQuery.trim() && messagesEl) {
      // Debounce to avoid excessive highlighting during streaming
      const timer = setTimeout(() => {
        performSearch(searchQuery);
      }, 300);
      return () => clearTimeout(timer);
    }
  });

  function requestClose() {
    if (closed) return;
    closed = true;
    dismissing = true;
    stopPolling();
    requestAnimationFrame(() => {
      onClose();
    });
  }

  /** Group consecutive tool_call / tool_result messages into pairs */
  type ToolPair = {
    call: SessionMessage;
    result: SessionMessage | null;
  };

  type MessageGroup =
    | { type: 'user'; message: SessionMessage }
    | { type: 'assistant'; message: SessionMessage }
    | { type: 'tools'; pairs: ToolPair[] };

  let grouped = $derived.by(() => {
    const groups: MessageGroup[] = [];
    let i = 0;
    while (i < messages.length) {
      const msg = messages[i];
      if (msg.role === 'user') {
        groups.push({ type: 'user', message: msg });
        i++;
      } else if (msg.role === 'assistant') {
        groups.push({ type: 'assistant', message: msg });
        i++;
      } else {
        // tool_call / tool_result: collect into pairs
        const pairs: ToolPair[] = [];
        while (
          i < messages.length &&
          (messages[i].role === 'tool_call' || messages[i].role === 'tool_result')
        ) {
          if (messages[i].role === 'tool_call') {
            const call = messages[i];
            i++;
            // Check if next message is the matching result
            let result: SessionMessage | null = null;
            if (i < messages.length && messages[i].role === 'tool_result') {
              result = messages[i];
              i++;
            }
            pairs.push({ call, result });
          } else {
            // Orphan tool_result (shouldn't happen, but handle gracefully)
            pairs.push({
              call: messages[i],
              result: null,
            });
            i++;
          }
        }
        groups.push({ type: 'tools', pairs });
      }
    }
    return groups;
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  class:dismissing
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
  onkeydown={(e) => e.key === 'Escape' && requestClose()}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()}>
    <!-- Header -->
    <header class="modal-header">
      <div class="header-content">
        <span class="header-title">
          {session?.prompt
            ? session.prompt.replace(/<(action|branch-history)>[\s\S]*?<\/\1>/g, '').trim() ||
              'Session'
            : 'Session'}
        </span>
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
        <button class="close-btn" onclick={requestClose} title="Close (Esc)">
          <X size={16} />
        </button>
      </div>
    </header>

    <!-- Messages area -->
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div
      class="modal-content"
      bind:this={messagesEl}
      onscroll={handleScroll}
      onclick={handleExternalLinkClick}
    >
      {#if loading}
        <div class="center-state">
          <Spinner size={24} />
          <span>Loading session…</span>
        </div>
      {:else if error}
        <div class="center-state error">
          <AlertCircle size={24} />
          <span>{error}</span>
        </div>
      {:else if grouped.length === 0 && isLive}
        <div class="center-state">
          <Spinner size={20} />
          <span>Waiting for response…</span>
        </div>
      {:else if grouped.length === 0}
        <div class="center-state">
          <span>No messages</span>
        </div>
      {:else}
        <div class="messages">
          {#each grouped as group}
            {#if group.type === 'user'}
              {@const hasBlocks = hasXmlBlocks(group.message.content)}
              {@const segments = hasBlocks ? parseContentSegments(group.message.content) : []}
              {@const userText = hasBlocks
                ? segments
                    .filter((s) => s.type === 'text')
                    .map((s) => s.text)
                    .join('\n')
                : group.message.content}
              {@const xmlBlocks = segments.filter((s) => s.type === 'xml-block')}
              <!-- Context cards (above the bubble, left-aligned like tool calls) -->
              {#if xmlBlocks.length > 0}
                <div class="message-row context-block-group">
                  {#each xmlBlocks as seg, segIdx}
                    {#if seg.type === 'xml-block'}
                      {@const blockKey = `${group.message.id}-${segIdx}`}
                      {@const isOpen = expandedXmlBlocks.has(blockKey)}
                      <div class="context-card">
                        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                        <div class="context-card-header" onclick={() => toggleXmlBlock(blockKey)}>
                          <span class="context-chevron">
                            {#if isOpen}
                              <ChevronDown size={12} />
                            {:else}
                              <ChevronRight size={12} />
                            {/if}
                          </span>
                          <seg.icon size={12} class="context-icon" />
                          <span class="context-label">{seg.label}</span>
                        </div>
                        {#if isOpen}
                          <div class="context-card-content">
                            <pre>{seg.content}</pre>
                          </div>
                        {/if}
                      </div>
                    {/if}
                  {/each}
                </div>
              {/if}
              <!-- User prompt bubble -->
              {#if userText.trim()}
                <div class="message-row human-message">
                  <div class="human-bubble">
                    {userText}
                    <button
                      class="copy-btn inline-copy"
                      onclick={() => copyContent(group.message.content, group.message.id)}
                      title="Copy message"
                    >
                      {#if copiedId === group.message.id}
                        <Check size={12} />
                      {:else}
                        <Copy size={12} />
                      {/if}
                    </button>
                  </div>
                </div>
              {/if}
            {:else if group.type === 'assistant'}
              <div class="message-row assistant-message">
                <div class="assistant-content">
                  <div class="markdown-content">
                    {@html renderMarkdown(group.message.content)}
                  </div>
                  <button
                    class="copy-btn"
                    onclick={() => copyContent(group.message.content, group.message.id)}
                    title="Copy message"
                  >
                    {#if copiedId === group.message.id}
                      <Check size={12} />
                    {:else}
                      <Copy size={12} />
                    {/if}
                  </button>
                </div>
              </div>
            {:else}
              <div class="message-row tool-group">
                {#each group.pairs as pair}
                  {@const toolName = formatToolName(pair.call.content)}
                  {@const toolArgs = formatToolArgs(pair.call.content)}
                  {@const isExpanded = expandedTools.has(pair.call.id)}
                  <div class="tool-card">
                    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                    <div
                      class="tool-header"
                      class:tool-header-expandable={!!pair.result}
                      onclick={() => pair.result && toggleTool(pair.call.id)}
                    >
                      <span
                        class="tool-caret"
                        class:tool-caret-expanded={isExpanded}
                        class:tool-caret-hidden={!pair.result}>›</span
                      >
                      <span class="tool-name">{toolName}</span>
                      {#if toolArgs}
                        <span class="tool-args-preview">{toolArgs}</span>
                      {/if}
                    </div>
                    {#if isExpanded && pair.result}
                      <div class="tool-output">
                        <pre>{pair.result.content}</pre>
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          {/each}

          {#if isLive}
            <div class="thinking">
              <Spinner size={14} />
              <span>Thinking…</span>
            </div>
          {/if}
        </div>
      {/if}

      {#if session?.status === 'error' && session.errorMessage}
        <div class="error-banner">
          <AlertCircle size={14} />
          <span>{session.errorMessage}</span>
        </div>
      {/if}
    </div>

    <!-- Input area with queued messages -->
    <div class="input-wrapper">
      {#if hasQueuedMessages}
        <div class="queue-popover">
          {#each messageQueue as msg, i}
            <div class="queue-item">
              <span class="queue-item-label">Queued</span>
              <span class="queue-item-text">{msg}</span>
              <button
                class="queue-item-remove"
                onclick={() => {
                  messageQueue = messageQueue.filter((_, idx) => idx !== i);
                }}
                title="Remove from queue"
              >
                <X size={10} />
              </button>
            </div>
          {/each}
        </div>
      {/if}
      <div class="input-area">
        <textarea
          bind:this={inputEl}
          bind:value={inputText}
          class="message-input"
          placeholder={isLive ? 'Type to queue a follow-up…' : 'Send a message…'}
          rows={1}
          onkeydown={handleInputKeydown}
          oninput={autoResize}
        ></textarea>
        {#if isLive}
          <button
            class="action-btn stop-btn"
            onclick={handleCancel}
            disabled={cancelling}
            title="Stop session"
          >
            {#if cancelling}
              <Spinner size={16} />
            {:else}
              <CircleStop size={16} />
            {/if}
          </button>
        {:else}
          <button
            class="action-btn send-btn"
            onclick={handleSend}
            disabled={!inputText.trim()}
            title="Send message"
          >
            <Send size={16} />
          </button>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  /* ======================================================================= */
  /* Modal shell                                                             */
  /* ======================================================================= */

  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal-backdrop.dismissing {
    opacity: 0;
    pointer-events: none;
  }

  .modal {
    display: flex;
    flex-direction: column;
    width: 700px;
    height: 600px;
    background: var(--bg-chrome);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: var(--shadow-elevated);
  }

  /* ----- Header ---------------------------------------------------------- */

  .modal-header {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }

  .header-title {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
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
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  /* ----- Content area (scrollable) --------------------------------------- */

  .modal-content {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    min-height: 0;
  }

  /* Custom scrollbar */
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

  /* ----- Center states --------------------------------------------------- */

  .center-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 40px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    height: 100%;
  }

  .center-state.error {
    color: var(--ui-danger);
  }

  /* ----- Messages -------------------------------------------------------- */

  .messages {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .message-row {
    position: relative;
  }

  /* Human messages — bubble, right-aligned */
  .human-message {
    display: flex;
    justify-content: flex-end;
  }

  .human-bubble {
    position: relative;
    max-width: 80%;
    padding: 8px 12px;
    padding-right: 30px;
    background: var(--bg-elevated, var(--bg-hover));
    border-radius: 14px 14px 4px 14px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.5;
    word-break: break-word;
    white-space: pre-wrap;
  }

  .human-bubble .inline-copy {
    position: absolute;
    top: 6px;
    right: 6px;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .human-bubble:hover .inline-copy {
    opacity: 1;
  }

  /* Context blocks (action/branch-history tags rendered as tool-call-style cards) */
  .context-block-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-left: 2px;
  }

  .context-card {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-primary);
  }

  .context-card-header {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .context-card-header:hover {
    background: var(--bg-hover);
  }

  .context-chevron {
    display: flex;
    align-items: center;
    width: 12px;
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .context-card-header :global(.context-icon) {
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .context-label {
    font-weight: 500;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) * 0.95);
  }

  .context-card-content {
    border-top: 1px solid var(--border-subtle);
    padding: 8px 10px;
    max-height: 200px;
    overflow-y: auto;
  }

  .context-card-content pre {
    margin: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.5;
  }

  .context-card-content::-webkit-scrollbar {
    width: 4px;
  }

  .context-card-content::-webkit-scrollbar-track {
    background: transparent;
  }

  .context-card-content::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 2px;
  }

  /* Assistant messages */
  .assistant-message {
    display: flex;
  }

  .assistant-content {
    position: relative;
    flex: 1;
    min-width: 0;
    padding-right: 28px;
  }

  .assistant-content > .copy-btn {
    position: absolute;
    top: 0;
    right: 0;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .assistant-content:hover > .copy-btn {
    opacity: 1;
  }

  /* Copy button base */
  .copy-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 3px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .copy-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  /* ----- Tool calls ------------------------------------------------------ */

  .tool-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    min-width: 0;
  }

  .tool-card {
    overflow: visible;
    min-width: 0;
  }

  .tool-header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
    background: none;
    color: var(--text-muted);
    font-size: var(--size-xs);
    transition: background-color 0.1s;
    cursor: default;
  }

  .tool-header-expandable {
    cursor: pointer;
  }

  .tool-header-expandable:hover .tool-name {
    text-decoration: underline;
  }

  .tool-caret {
    display: inline-block;
    flex-shrink: 0;
    width: 8px;
    margin-left: -8px;
    font-size: var(--size-xs);
    color: var(--text-faint);
    transition: transform 0.15s ease;
    line-height: 1;
  }

  .tool-caret-expanded {
    transform: rotate(90deg);
  }

  .tool-caret-hidden {
    visibility: hidden;
  }

  .tool-name {
    flex: 1;
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-args-preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .tool-output {
    padding: 4px 0 4px 14px;
    max-height: 200px;
    overflow-y: auto;
  }

  .tool-output pre {
    margin: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.5;
  }

  .tool-output::-webkit-scrollbar {
    width: 4px;
  }

  .tool-output::-webkit-scrollbar-track {
    background: transparent;
  }

  .tool-output::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 2px;
  }

  /* Thinking indicator */
  .thinking {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    padding: 4px 0;
  }

  /* Error banner */
  .error-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
    padding: 8px 12px;
    background: var(--ui-danger-bg, rgba(248, 81, 73, 0.1));
    color: var(--ui-danger);
    border-radius: 8px;
    font-size: var(--size-xs);
    line-height: 1.4;
  }

  /* ----- Input wrapper + queue popover ----------------------------------- */

  .input-wrapper {
    flex-shrink: 0;
  }

  .queue-popover {
    display: flex;
    flex-direction: column;
    gap: 0;
    margin: 0 14px;
    border: 1px solid var(--border-muted);
    border-bottom: none;
    border-radius: 10px 10px 0 0;
    background: var(--bg-elevated);
    overflow: hidden;
  }

  .queue-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    font-size: calc(var(--size-xs) * 0.95);
    line-height: 1.4;
    border-bottom: 1px solid var(--border-muted);
  }

  .queue-item:last-child {
    border-bottom: none;
  }

  .queue-item-label {
    flex-shrink: 0;
    font-size: calc(var(--size-xs) * 0.85);
    font-weight: 500;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .queue-item-text {
    flex: 1;
    min-width: 0;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .queue-item-remove {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    padding: 0;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .queue-item-remove:hover {
    color: var(--ui-danger);
    background: var(--bg-hover);
  }

  /* ----- Input area ------------------------------------------------------ */

  .input-area {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 10px 14px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    flex-shrink: 0;
  }

  .message-input {
    flex: 1;
    padding: 8px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
    overflow-y: auto;
    min-height: 36px;
    max-height: 120px;
  }

  .message-input::placeholder {
    color: var(--text-faint);
  }

  .message-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    padding: 0;
    border: none;
    border-radius: 10px;
    cursor: pointer;
    flex-shrink: 0;
    transition:
      background-color 0.15s,
      opacity 0.15s;
  }

  .send-btn {
    background: var(--ui-accent);
    color: var(--bg-deepest);
  }

  .send-btn:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .send-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .stop-btn {
    background: var(--ui-danger-bg, rgba(248, 81, 73, 0.15));
    color: var(--ui-danger);
  }

  .stop-btn:hover:not(:disabled) {
    background: var(--ui-danger);
    color: white;
  }

  .stop-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* ======================================================================= */
  /* Markdown content (assistant messages)                                   */
  /* ======================================================================= */

  .markdown-content {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.6;
  }

  .markdown-content :global(p) {
    margin: 0 0 0.5em 0;
  }

  .markdown-content :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3),
  .markdown-content :global(h4) {
    margin: 0.75em 0 0.5em 0;
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
    margin: 0.5em 0;
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
    margin: 0.75em 0;
    border: none;
    border-top: 1px solid var(--border-subtle);
  }
</style>
