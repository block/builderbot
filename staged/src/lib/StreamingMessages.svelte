<!--
  StreamingMessages.svelte - Shared component for rendering AI session messages

  Design principles (matching reference):
  - Human messages: gray bubble, right-aligned
  - Assistant messages: no bubble, plain text
  - Tool calls: minimal inline text like "Ran `command`"
-->
<script lang="ts">
  import { Loader2 } from 'lucide-svelte';
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

  // Format tool title for minimal inline display
  function formatToolDisplay(title: string): { prefix: string; command: string | null } {
    // Common patterns: "Ran command", "Read file.ts", "Edited file.ts", etc.
    const ranMatch = title.match(/^Ran\s+(.+)$/i);
    if (ranMatch) {
      return { prefix: 'Ran', command: ranMatch[1] };
    }

    const readMatch = title.match(/^Read\s+(.+)$/i);
    if (readMatch) {
      return { prefix: 'Read', command: readMatch[1] };
    }

    const editedMatch = title.match(/^Edited?\s+(.+)$/i);
    if (editedMatch) {
      return { prefix: 'Edited', command: editedMatch[1] };
    }

    const viewedMatch = title.match(/^Viewed?\s+(.+)$/i);
    if (viewedMatch) {
      return { prefix: 'Viewed', command: viewedMatch[1] };
    }

    const exploredMatch = title.match(/^Explored?\s+(.+)$/i);
    if (exploredMatch) {
      return { prefix: 'Explored', command: exploredMatch[1] };
    }

    const analyzedMatch = title.match(/^Analyzed?\s+(.+)$/i);
    if (analyzedMatch) {
      return { prefix: 'Analyzed', command: analyzedMatch[1] };
    }

    // Default: just show the title as-is
    return { prefix: title, command: null };
  }
</script>

<div class="messages">
  {#each messages as message}
    {#if message.role === 'user'}
      <!-- Human message: bubble style, right-aligned -->
      <div class="human-message">
        <div class="human-bubble">
          {message.content}
        </div>
      </div>
    {:else}
      <!-- Assistant message: no bubble, natural flow -->
      <div class="assistant-message">
        {#each message.segments as segment}
          {#if segment.type === 'text'}
            <div class="assistant-text markdown-content">
              {@html renderMarkdown(segment.text)}
            </div>
          {:else}
            {@const display = formatToolDisplay(segment.title)}
            <div class="tool-call">
              <span class="tool-prefix">{display.prefix}</span>
              {#if display.command}
                <code>{display.command}</code>
              {/if}
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  {/each}

  <!-- Streaming content -->
  {#if isActive && streamingSegments.length > 0}
    <div class="assistant-message">
      {#each streamingSegments as segment, i}
        {#if segment.type === 'text'}
          <div class="assistant-text streaming-text">
            {segment.text}{#if i === streamingSegments.length - 1}<span class="cursor">▋</span>{/if}
          </div>
        {:else}
          {@const display = formatToolDisplay(segment.title)}
          <div class="tool-call" class:running={segment.status === 'running'}>
            {#if segment.status === 'running'}
              <Loader2 size={12} class="spinning" />
            {/if}
            <span class="tool-prefix">{display.prefix}</span>
            {#if display.command}
              <code>{display.command}</code>
            {/if}
          </div>
        {/if}
      {/each}
    </div>
  {:else if isActive}
    <div class="assistant-message">
      <div class="thinking">
        <Loader2 size={14} class="spinning" />
        <span>{waitingText}</span>
      </div>
    </div>
  {/if}
</div>

<style>
  .messages {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  /* Human message - bubble style, right-aligned */
  .human-message {
    display: flex;
    justify-content: flex-end;
  }

  .human-bubble {
    max-width: 85%;
    padding: 10px 14px;
    background: var(--bg-elevated);
    border-radius: 18px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.5;
    word-break: break-word;
    white-space: pre-wrap;
  }

  /* Assistant message - no bubble, natural flow */
  .assistant-message {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .assistant-text {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.6;
  }

  .streaming-text {
    white-space: pre-wrap;
  }

  .thinking {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
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

  /* Tool calls - minimal inline style like "Ran `pwd`" */
  .tool-call {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .tool-call.running {
    color: var(--text-accent);
  }

  .tool-prefix {
    color: var(--text-faint);
  }

  .tool-call code {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  :global(.spinning) {
    animation: spin 1s linear infinite;
    transform-origin: center;
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
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: 0.9em;
    background: var(--bg-elevated);
    padding: 0.15em 0.35em;
    border-radius: 3px;
  }

  .markdown-content :global(pre) {
    margin: 0.5em 0;
    padding: 0.75em;
    background: var(--bg-elevated);
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
