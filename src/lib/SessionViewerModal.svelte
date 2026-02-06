<!--
  SessionViewerModal.svelte - View an AI session (live or historical)

  Shows the conversation with tool calls and text. For running sessions,
  subscribes to streaming events for real-time updates via the shared store.
-->
<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { X, Loader2, AlertCircle } from 'lucide-svelte';
  import { getSession, getSessionStatus, getBufferedSegments, type ContentSegment } from './services/ai';
  import { toDisplayMessage, type DisplayMessage } from './types/streaming';
  import {
    connectToSession,
    disconnectFromSession,
    clearStreamingState,
    type StreamingSessionState,
    type ConnectOptions,
  } from './stores/streamingSession.svelte';
  import StreamingMessages from './StreamingMessages.svelte';

  interface Props {
    /** The AI session ID to display */
    sessionId: string;
    /** Title to show in the header (e.g., the prompt) */
    title?: string;
    /** Whether this is a live session (subscribes to streaming events) */
    isLive?: boolean;
    onClose: () => void;
  }

  let { sessionId, title, isLive = true, onClose }: Props = $props();

  // ==========================================================================
  // State
  // ==========================================================================

  let messages = $state<DisplayMessage[]>([]);
  let loading = $state(true);
  let isProcessing = $state(false);
  let error = $state<string | null>(null);

  // Streaming store connection
  let streamState = $state<StreamingSessionState | null>(null);
  let connectOptions: ConnectOptions | undefined;

  // Refs
  let messagesContainer: HTMLDivElement;

  // ==========================================================================
  // Lifecycle
  // ==========================================================================

  onMount(async () => {
    if (isLive) {
      connectOptions = {
        onIdle: () => refreshFromDatabase(),
        onError: (message) => {
          error = message;
          isProcessing = false;
        },
      };
      streamState = connectToSession(sessionId, connectOptions);
    }

    await loadSession();
  });

  onDestroy(() => {
    if (isLive) {
      disconnectFromSession(sessionId, connectOptions);
    }
  });

  async function loadSession() {
    loading = true;
    error = null;

    try {
      // Load from both sources concurrently
      const [sessionData, bufferedSegments] = await Promise.all([
        getSession(sessionId),
        isLive ? getBufferedSegments(sessionId) : Promise.resolve(null),
      ]);

      if (!sessionData) {
        error = 'Session not found';
        return;
      }

      // Convert DB messages to display format
      const dbMessages = sessionData.messages.map(toDisplayMessage);

      // If we have buffered segments, merge them with DB messages
      if (bufferedSegments && bufferedSegments.length > 0) {
        // Convert buffered segments to a display message
        const bufferedMessage: DisplayMessage = {
          role: 'assistant',
          content: '',
          segments: bufferedSegments.map((seg: ContentSegment) => {
            if (seg.type === 'text') {
              return { type: 'text' as const, text: seg.text };
            } else {
              return {
                type: 'tool' as const,
                id: seg.id,
                title: seg.title,
                status: seg.status,
              };
            }
          }),
        };

        // Dedupe: if the last DB message is assistant AND recent (within last 5 seconds),
        // assume it's the persisted version of the buffered segments
        const lastDbMessage = dbMessages[dbMessages.length - 1];
        const lastDbRawMessage = sessionData.messages[sessionData.messages.length - 1];
        const isRecentAssistant =
          lastDbMessage?.role === 'assistant' &&
          lastDbRawMessage &&
          (Date.now() - lastDbRawMessage.createdAt) < 5000;

        if (isRecentAssistant) {
          // DB has the persisted version (it's recent), use it (buffered is stale)
          messages = dbMessages;
        } else {
          // No recent assistant message in DB, use buffered
          messages = [...dbMessages, bufferedMessage];
        }
      } else {
        messages = dbMessages;
      }

      if (isLive) {
        const status = await getSessionStatus(sessionId);
        isProcessing = status.status === 'processing';
        if (streamState) {
          streamState.isActive = isProcessing;
        }
      } else {
        isProcessing = false;
      }

      scrollToBottom();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  // ==========================================================================
  // Helpers
  // ==========================================================================

  async function refreshFromDatabase() {
    try {
      const sessionData = await getSession(sessionId);
      if (sessionData) {
        messages = sessionData.messages.map(toDisplayMessage);
      }
    } catch (e) {
      console.error('Failed to refresh from database:', e);
    }

    clearStreamingState(sessionId);
    isProcessing = false;
    scrollToBottom();
  }

  function scrollToBottom() {
    tick().then(() => {
      if (messagesContainer) {
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
      }
    });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      onClose();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="modal-backdrop" role="presentation" onclick={onClose}>
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="modal"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <header class="modal-header">
      <div class="header-content">
        <span class="header-title">{title || 'Session'}</span>
        {#if isProcessing}
          <span class="status-badge">
            <Loader2 size={12} class="spinning" />
            Running
          </span>
        {/if}
      </div>
      <button class="close-btn" onclick={onClose}>
        <X size={18} />
      </button>
    </header>

    <div class="modal-content" bind:this={messagesContainer}>
      {#if loading}
        <div class="loading-state">
          <Loader2 size={24} class="spinning" />
          <span>Loading session...</span>
        </div>
      {:else if error}
        <div class="error-state">
          <AlertCircle size={24} />
          <span>{error}</span>
        </div>
      {:else if messages.length === 0 && !isProcessing}
        <div class="empty-state">
          <span>No messages yet</span>
        </div>
      {:else}
        <StreamingMessages
          {messages}
          streamingSegments={streamState?.streamingSegments ?? []}
          isActive={isProcessing}
        />
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    display: flex;
    flex-direction: column;
    width: 90%;
    max-width: 700px;
    max-height: 80vh;
    background: var(--bg-chrome);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 20px 40px rgba(0, 0, 0, 0.3);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-primary);
    min-width: 0;
    flex: 1;
  }

  .header-title {
    font-size: var(--size-md);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: var(--size-xs);
    font-weight: 500;
    flex-shrink: 0;
    background: var(--ui-accent);
    color: var(--bg-deepest);
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
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

  .modal-content {
    flex: 1;
    overflow-y: auto;
    padding: 20px;
  }

  .loading-state,
  .error-state,
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 40px;
    color: var(--text-muted);
  }

  .error-state {
    color: var(--ui-danger);
  }

  :global(.spinning) {
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
</style>
