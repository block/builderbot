<!--
  StreamingMessages.svelte - Shared component for rendering AI session messages

  Pure presentational component. Renders persisted messages from the database
  and live streaming segments from the streaming store.
-->
<script lang="ts">
  import { Bot, User, Loader2, Wrench } from 'lucide-svelte';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import type { DisplayMessage, DisplaySegment } from './types/streaming';

  // Configure marked for safe rendering
  marked.setOptions({
    breaks: true,
    gfm: true,
  });

  interface Props {
    /** Persisted messages from the database */
    messages: DisplayMessage[];
    /** Live streaming segments from the streaming store */
    streamingSegments: DisplaySegment[];
    /** Whether the session is currently streaming */
    isActive: boolean;
    /** Customizable waiting text (default: "Thinking...") */
    waitingText?: string;
  }

  let { messages, streamingSegments, isActive, waitingText = 'Thinking...' }: Props = $props();

  // Render markdown content safely
  function renderMarkdown(content: string): string {
    return DOMPurify.sanitize(marked.parse(content) as string);
  }
</script>

{#each messages as message}
  <div class="message" class:user={message.role === 'user'}>
    <div class="message-icon">
      {#if message.role === 'user'}
        <User size={14} />
      {:else}
        <Bot size={14} />
      {/if}
    </div>
    <div class="message-content">
      {#if message.role === 'user'}
        <div class="message-text">{message.content}</div>
      {:else}
        {#each message.segments as segment}
          {#if segment.type === 'text'}
            <div class="message-text markdown-content">
              {@html renderMarkdown(segment.text)}
            </div>
          {:else}
            <div class="tool-call" class:completed={segment.status === 'completed'}>
              <Wrench size={12} />
              <span class="tool-title">{segment.title}</span>
            </div>
          {/if}
        {/each}
      {/if}
    </div>
  </div>
{/each}

<!-- Streaming content -->
{#if isActive && streamingSegments.length > 0}
  <div class="message">
    <div class="message-icon">
      <Bot size={14} />
    </div>
    <div class="message-content">
      {#each streamingSegments as segment, i}
        {#if segment.type === 'text'}
          <div class="message-text streaming-text">
            {segment.text}{#if i === streamingSegments.length - 1}<span class="cursor">▋</span>{/if}
          </div>
        {:else}
          <div
            class="tool-call"
            class:running={segment.status === 'running'}
            class:completed={segment.status === 'completed'}
          >
            {#if segment.status === 'running'}
              <Loader2 size={12} class="spinning" />
            {:else}
              <Wrench size={12} />
            {/if}
            <span class="tool-title">{segment.title}</span>
          </div>
        {/if}
      {/each}
    </div>
  </div>
{:else if isActive}
  <div class="message">
    <div class="message-icon">
      <Bot size={14} />
    </div>
    <div class="message-content">
      <div class="message-text thinking">
        <Loader2 size={14} class="spinning" />
        <span>{waitingText}</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .message {
    display: flex;
    gap: 10px;
  }

  .message.user {
    flex-direction: row-reverse;
  }

  .message-icon {
    flex-shrink: 0;
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-primary);
    border-radius: 50%;
    color: var(--text-muted);
  }

  .message.user .message-icon {
    background: var(--ui-accent);
    color: var(--bg-primary);
  }

  .message-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .message.user .message-content {
    align-items: flex-end;
  }

  .message-text {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.5;
    word-break: break-word;
  }

  /* Streaming text keeps pre-wrap for live updates */
  .message-text.streaming-text {
    white-space: pre-wrap;
  }

  .message.user .message-text {
    background: var(--ui-accent);
    color: var(--bg-primary);
    padding: 8px 12px;
    border-radius: 12px 12px 4px 12px;
    max-width: 85%;
  }

  .message:not(.user) .message-text {
    background: var(--bg-primary);
    padding: 8px 12px;
    border-radius: 12px 12px 12px 4px;
    max-width: 85%;
  }

  .message-text.thinking {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
  }

  .cursor {
    animation: blink 1s step-end infinite;
    color: var(--text-muted);
  }

  @keyframes blink {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0;
    }
  }

  .tool-call {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    padding: 4px 8px;
    background: var(--bg-primary);
    border-radius: 4px;
    border: 1px solid var(--border-subtle);
  }

  .tool-call.running {
    border-color: var(--text-accent);
  }

  .tool-call.completed {
    border-color: var(--ui-accent);
  }

  .tool-call :global(svg) {
    flex-shrink: 0;
  }

  .tool-title {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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

  /* Markdown content styles */
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
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
    font-size: 0.9em;
    background: var(--bg-hover);
    padding: 0.15em 0.35em;
    border-radius: 3px;
  }

  .markdown-content :global(pre) {
    margin: 0.5em 0;
    padding: 0.75em;
    background: var(--bg-hover);
    border-radius: 4px;
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
    color: var(--text-accent);
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
