<script lang="ts">
  import type { StructuralDeclaration } from '../utils/structuralHeaders';

  interface Props {
    stack?: StructuralDeclaration[];
    maxRows?: number;
  }

  let { stack = [], maxRows = 5 }: Props = $props();

  const INDENT_WIDTH = 6;
  const BASE_PADDING = 12;
  const MAX_INDENT_PADDING = 72;

  let visibleStack = $derived(stack.slice(-maxRows));

  function paddingForIndent(indent: number): number {
    return BASE_PADDING + Math.min(indent * INDENT_WIDTH, MAX_INDENT_PADDING);
  }
</script>

{#if visibleStack.length > 0}
  <div class="structural-header-stack" aria-label="Current code scope">
    {#each visibleStack as declaration (declaration.lineIndex)}
      <div
        class="structural-header-row"
        style="padding-left: {paddingForIndent(declaration.indent)}px"
        title={declaration.displayText}
      >
        <span class="structural-header-text">{declaration.displayText}</span>
      </div>
    {/each}
  </div>
{/if}

<style>
  .structural-header-stack {
    position: absolute;
    top: 0;
    left: 0;
    right: 12px;
    z-index: 20;
    padding: 2px 0;
    border-top: 1px solid var(--border-subtle);
    border-bottom: 1px solid var(--border-subtle);
    background-color: color-mix(in srgb, var(--bg-primary) 88%, var(--bg-elevated));
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
    pointer-events: none;
  }

  .structural-header-row {
    display: flex;
    align-items: center;
    min-height: 18px;
    padding-right: 12px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    line-height: 1.35;
    white-space: nowrap;
    overflow: hidden;
  }

  .structural-header-row:last-child {
    color: var(--text-primary);
  }

  .structural-header-text {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
