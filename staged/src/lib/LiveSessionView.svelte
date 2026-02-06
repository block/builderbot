<!--
  LiveSessionView.svelte - Renders a streaming or completed session

  Shows:
  - Completed messages (rendered as markdown)
  - Active tool calls (ToolCallCard components)
  - Current streaming text with cursor
  - Error state if session failed
-->
<script lang="ts">
  import { AlertCircle, Loader2 } from 'lucide-svelte';
  import type { LiveSession } from './stores/liveSession.svelte';
  import type { FinalizedMessage } from './services/ai';
  import ToolCallCard from './ToolCallCard.svelte';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';

  interface Props {
    session: LiveSession;
  }

  let { session }: Props = $props();

  // Render markdown for completed messages
  function renderMarkdown(content: string): string {
    return DOMPurify.sanitize(marked(content) as string);
  }

  // Convert tool calls map to array for iteration
  let activeToolCalls = $derived([...session.toolCalls.values()]);
</script>

<div class="live-session">
  {#if session.error}
    <!-- Error state -->
    <div class="error-state">
      <AlertCircle size={20} />
      <span>Session failed: {session.error}</span>
    </div>
  {:else if session.finalTranscript}
    <!-- Session complete - show final transcript -->
    {#each session.finalTranscript as message}
      <div class="message {message.role}">
        {#if message.role === 'user'}
          <div class="message-header">
            <span class="message-role">You</span>
          </div>
          <div class="user-bubble">{message.content}</div>
        {:else}
          <div class="message-header">
            <span class="message-role">AI</span>
          </div>
          <div class="assistant-content">
            {@html renderMarkdown(message.content)}
          </div>
          {#if message.toolCalls?.length}
            <div class="tool-calls-summary">
              {#each message.toolCalls as tc}
                <ToolCallCard
                  tool={{
                    id: tc.id,
                    title: tc.title,
                    status:
                      (tc.status as 'pending' | 'in_progress' | 'completed' | 'failed') ??
                      'completed',
                    kind: 'other',
                    locations: tc.locations ?? [],
                    preview: tc.resultPreview,
                  }}
                />
              {/each}
            </div>
          {/if}
        {/if}
      </div>
    {/each}
  {:else}
    <!-- Session streaming - show live state -->

    <!-- Active tool calls -->
    {#if activeToolCalls.length > 0}
      <div class="active-tools">
        {#each activeToolCalls as tool (tool.id)}
          <ToolCallCard {tool} />
        {/each}
      </div>
    {/if}

    <!-- Streaming text -->
    {#if session.currentText}
      <div class="streaming-text">
        {session.currentText}<span class="cursor">▊</span>
      </div>
    {:else if session.isStreaming && activeToolCalls.length === 0}
      <div class="waiting">
        <Loader2 size={16} class="spinner" />
        <span>Waiting for response...</span>
      </div>
    {/if}
  {/if}
</div>

<style>
  .live-session {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .error-state {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: rgba(248, 81, 73, 0.1);
    border-radius: 8px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .message {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .message-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .message-role {
    font-size: var(--size-sm);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .message.user .message-role {
    color: var(--text-accent);
  }

  .message.assistant .message-role {
    color: var(--status-added);
  }

  .user-bubble {
    background: var(--bg-elevated);
    padding: 12px 16px;
    border-radius: 8px;
    border-left: 3px solid var(--text-accent);
    white-space: pre-wrap;
    font-size: var(--size-md);
    line-height: 1.6;
  }

  .assistant-content {
    line-height: 1.6;
    color: var(--text-primary);
    border-left: 3px solid var(--status-added);
    padding-left: 16px;
  }

  .tool-calls-summary {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 8px;
    margin-left: 16px;
  }

  .active-tools {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .streaming-text {
    line-height: 1.6;
    color: var(--text-primary);
    white-space: pre-wrap;
    font-size: var(--size-md);
  }

  .cursor {
    color: var(--text-accent);
    animation: blink 1s step-end infinite;
  }

  @keyframes blink {
    50% {
      opacity: 0;
    }
  }

  .waiting {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  /* Markdown content styles */
  .assistant-content :global(h1) {
    font-size: var(--size-xl);
    font-weight: 600;
    margin: 0 0 16px 0;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .assistant-content :global(h2) {
    font-size: var(--size-lg);
    font-weight: 600;
    margin: 24px 0 12px 0;
  }

  .assistant-content :global(h3) {
    font-size: var(--size-md);
    font-weight: 600;
    margin: 20px 0 8px 0;
  }

  .assistant-content :global(p) {
    margin: 0 0 12px 0;
  }

  .assistant-content :global(ul),
  .assistant-content :global(ol) {
    margin: 0 0 12px 0;
    padding-left: 24px;
  }

  .assistant-content :global(li) {
    margin: 4px 0;
  }

  .assistant-content :global(code) {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-sm);
    background-color: var(--bg-elevated);
    padding: 2px 6px;
    border-radius: 4px;
  }

  .assistant-content :global(pre) {
    background-color: var(--bg-deepest);
    border-radius: 8px;
    padding: 16px;
    overflow-x: auto;
    margin: 12px 0;
  }

  .assistant-content :global(pre code) {
    background: none;
    padding: 0;
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
</style>
