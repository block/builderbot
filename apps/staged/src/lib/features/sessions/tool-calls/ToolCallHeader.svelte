<script lang="ts">
  import type { RichToolItem } from '../acpTranscript';
  import ToolStatusDot from './ToolStatusDot.svelte';

  interface Props {
    verb: string;
    detail?: string;
    statusTone: RichToolItem['statusTone'];
    expanded?: boolean;
    expandable?: boolean;
    onToggle?: () => void;
  }

  let {
    verb,
    detail = '',
    statusTone,
    expanded = false,
    expandable = true,
    onToggle,
  }: Props = $props();
</script>

<button
  type="button"
  class="tool-header"
  class:tool-header-expandable={expandable}
  disabled={!expandable}
  aria-expanded={expandable ? expanded : undefined}
  onclick={() => {
    if (expandable) onToggle?.();
  }}
>
  <span
    class="tool-caret"
    class:tool-caret-expanded={expanded}
    class:tool-caret-hidden={!expandable}>›</span
  >
  <ToolStatusDot {statusTone} />
  <span class="tool-name">{verb}</span>
  {#if detail}
    <span class="tool-args-preview">{detail}</span>
  {/if}
</button>

<style>
  .tool-header {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    min-width: 0;
    border: 0;
    padding: 2px 0;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: var(--size-xs);
    text-align: left;
    transition: background-color 0.1s;
    cursor: default;
    opacity: 1;
  }

  .tool-header-expandable {
    cursor: pointer;
  }

  .tool-header-expandable:hover .tool-name {
    text-decoration: underline;
  }

  .tool-header:focus-visible {
    outline: 1px solid var(--border-emphasis);
    outline-offset: 2px;
    border-radius: 4px;
  }

  .tool-caret {
    display: inline-block;
    flex-shrink: 0;
    width: 8px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    line-height: 1;
    transition: transform 0.15s ease;
  }

  .tool-caret-expanded {
    transform: rotate(90deg);
  }

  .tool-caret-hidden {
    visibility: hidden;
  }

  .tool-name {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
    white-space: nowrap;
  }

  .tool-args-preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
