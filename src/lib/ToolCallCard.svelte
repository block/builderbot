<!--
  ToolCallCard.svelte - Compact card showing tool execution status

  Displays:
  - Tool icon based on kind (read, edit, execute, etc.)
  - Status indicator (spinner for running, checkmark for complete, X for failed)
  - Tool title
  - File locations (if any)
  - Optional result preview (expandable)
-->
<script lang="ts">
  import {
    FileText,
    Terminal,
    Pencil,
    Search,
    Trash2,
    FolderInput,
    Globe,
    Brain,
    ArrowRightLeft,
    Loader2,
    Check,
    X,
    ChevronDown,
  } from 'lucide-svelte';
  import type { LiveToolCall } from './stores/liveSession.svelte';
  type ToolKind =
    | 'read'
    | 'edit'
    | 'delete'
    | 'move'
    | 'search'
    | 'execute'
    | 'think'
    | 'fetch'
    | 'switch_mode'
    | 'other';

  interface Props {
    tool: LiveToolCall;
  }

  let { tool }: Props = $props();
  let expanded = $state(false);

  // Map tool kinds to icons
  const kindIcons: Record<ToolKind, typeof FileText> = {
    read: FileText,
    edit: Pencil,
    delete: Trash2,
    move: FolderInput,
    search: Search,
    execute: Terminal,
    think: Brain,
    fetch: Globe,
    switch_mode: ArrowRightLeft,
    other: FileText,
  };

  let Icon = $derived(kindIcons[tool.kind as ToolKind] ?? FileText);
  let isRunning = $derived(tool.status === 'pending' || tool.status === 'in_progress');
  let isComplete = $derived(tool.status === 'completed');
  let isFailed = $derived(tool.status === 'failed');
</script>

<div
  class="tool-card"
  class:running={isRunning}
  class:complete={isComplete}
  class:failed={isFailed}
>
  <div class="tool-icon">
    {#if isRunning}
      <Loader2 size={14} class="spinner" />
    {:else if isComplete}
      <Check size={14} />
    {:else if isFailed}
      <X size={14} />
    {:else}
      <Icon size={14} />
    {/if}
  </div>

  <div class="tool-info">
    <span class="tool-title">{tool.title}</span>
    {#if tool.locations.length > 0}
      <span class="tool-location">
        {tool.locations[0]}
        {#if tool.locations.length > 1}
          <span class="more">+{tool.locations.length - 1}</span>
        {/if}
      </span>
    {/if}
  </div>

  {#if tool.preview && !isRunning}
    <button
      class="expand-btn"
      onclick={() => (expanded = !expanded)}
      title={expanded ? 'Hide result' : 'Show result'}
    >
      <span class:rotated={expanded}>
        <ChevronDown size={12} />
      </span>
    </button>
  {/if}
</div>

{#if expanded && tool.preview}
  <div class="preview-container">
    <pre class="preview">{tool.preview}</pre>
  </div>
{/if}

<style>
  .tool-card {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-elevated);
    border-radius: 6px;
    border-left: 2px solid var(--border-subtle);
    font-size: var(--size-sm);
  }

  .tool-card.running {
    border-left-color: var(--text-accent);
  }

  .tool-card.complete {
    border-left-color: var(--status-added);
  }

  .tool-card.failed {
    border-left-color: var(--ui-danger);
  }

  .tool-icon {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .tool-card.running .tool-icon {
    color: var(--text-accent);
  }

  .tool-card.complete .tool-icon {
    color: var(--status-added);
  }

  .tool-card.failed .tool-icon {
    color: var(--ui-danger);
  }

  .tool-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .tool-title {
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tool-location {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tool-location .more {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .expand-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    flex-shrink: 0;
    transition: all 0.15s ease;
  }

  .expand-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .expand-btn .rotated {
    display: inline-flex;
    transform: rotate(180deg);
  }

  .expand-btn span {
    display: inline-flex;
    transition: transform 0.15s ease;
  }

  .preview-container {
    margin-top: 4px;
    margin-left: 24px;
  }

  .preview {
    margin: 0;
    padding: 8px 10px;
    background: var(--bg-deepest);
    border-radius: 4px;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 150px;
    overflow: auto;
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
