<script lang="ts">
  import UnfoldVertical from '@lucide/svelte/icons/unfold-vertical';
  import type { CharHighlight } from '@builderbot/diff-viewer/utils';
  import type { ToolCallDiff } from '../toolCallViewModel';
  import { buildDiffRows, diffRowStats, groupDiffRows, type DiffRow } from './inlineDiffRows';

  interface Segment {
    text: string;
    highlighted: boolean;
  }

  interface Props {
    diff: ToolCallDiff;
  }

  let { diff }: Props = $props();
  let rows = $derived.by(() => buildDiffRows(diff));
  let stats = $derived(diffRowStats(rows));
  let blocks = $derived.by(() => groupDiffRows(rows));
  let expandedKeys = $state<string[]>([]);

  function expand(key: string) {
    if (!expandedKeys.includes(key)) expandedKeys = [...expandedKeys, key];
  }

  function segments(text: string, highlights: CharHighlight[]): Segment[] {
    if (highlights.length === 0) return [{ text, highlighted: false }];
    const result: Segment[] = [];
    let cursor = 0;
    for (const highlight of highlights) {
      if (highlight.start > cursor) {
        result.push({ text: text.slice(cursor, highlight.start), highlighted: false });
      }
      if (highlight.end > highlight.start) {
        result.push({ text: text.slice(highlight.start, highlight.end), highlighted: true });
      }
      cursor = Math.max(cursor, highlight.end);
    }
    if (cursor < text.length) {
      result.push({ text: text.slice(cursor), highlighted: false });
    }
    return result;
  }

  function highlightedLineHtml(text: string, highlights: CharHighlight[]): string {
    return segments(text, highlights)
      .map((segment) => {
        const escaped = escapeHtml(segment.text);
        return segment.highlighted ? `<span class="char-highlight">${escaped}</span>` : escaped;
      })
      .join('');
  }

  function escapeHtml(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }
</script>

{#snippet diffRow(row: DiffRow)}
  <div
    class="diff-row"
    class:removed={row.tone === 'removed' || row.tone === 'modified-removed'}
    class:added={row.tone === 'added' || row.tone === 'modified-added'}
  >
    <span class="line-number old">{row.oldLine ?? ''}</span>
    <span class="line-number new">{row.newLine ?? ''}</span>
    <span class="marker">{row.marker}</span>
    <code class="line-text">{@html highlightedLineHtml(row.text, row.highlights)}</code>
  </div>
{/snippet}

<div class="inline-diff" aria-label={`${diff.path} diff`}>
  <div class="inline-diff-header">
    <span class="diff-path">{diff.path}</span>
    {#if diff.kind === 'created'}
      <span class="diff-kind-badge created">Created</span>
    {:else if diff.kind === 'deleted'}
      <span class="diff-kind-badge deleted">Deleted</span>
    {/if}
    {#if stats.added > 0 || stats.removed > 0}
      <span class="diff-stats">
        {#if stats.added > 0}<span class="diff-stat-added">+{stats.added}</span>{/if}
        {#if stats.removed > 0}<span class="diff-stat-removed">−{stats.removed}</span>{/if}
      </span>
    {/if}
  </div>
  {#if rows.length === 0}
    <div class="inline-diff-empty">No changes</div>
  {:else}
    {#each blocks as block (block.key)}
      {#if block.kind === 'visible' || expandedKeys.includes(block.key)}
        {#each block.rows as row (row.key)}
          {@render diffRow(row)}
        {/each}
      {:else}
        <button type="button" class="diff-collapse-row" onclick={() => expand(block.key)}>
          <UnfoldVertical size={11} />
          {block.rows.length} unchanged {block.rows.length === 1 ? 'line' : 'lines'}
        </button>
      {/if}
    {/each}
  {/if}
</div>

<style>
  .inline-diff {
    overflow: auto;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-primary);
  }

  .inline-diff-header {
    position: sticky;
    left: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border-subtle);
    background: color-mix(in srgb, var(--bg-chrome) 60%, var(--bg-primary));
  }

  .diff-path {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    color: var(--text-muted);
    font-weight: 600;
  }

  .diff-kind-badge {
    flex-shrink: 0;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    padding: 1px 6px;
    font-size: calc(var(--size-xs) * 0.85);
  }

  .diff-kind-badge.created {
    border-color: color-mix(in srgb, var(--ui-success, var(--ui-accent)) 35%, var(--border-subtle));
    color: var(--ui-success, var(--ui-accent));
  }

  .diff-kind-badge.deleted {
    border-color: color-mix(in srgb, var(--ui-danger) 35%, var(--border-subtle));
    color: var(--ui-danger);
  }

  .diff-stats {
    display: flex;
    flex-shrink: 0;
    gap: 6px;
  }

  .diff-stat-added {
    color: var(--ui-success, var(--ui-accent));
  }

  .diff-stat-removed {
    color: var(--ui-danger);
  }

  .inline-diff-empty {
    padding: 8px 10px;
    color: var(--text-faint);
  }

  .diff-collapse-row {
    position: sticky;
    left: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    width: 100%;
    border: 0;
    border-top: 1px dashed var(--border-subtle);
    border-bottom: 1px dashed var(--border-subtle);
    padding: 3px 8px;
    background: color-mix(in srgb, var(--bg-chrome) 40%, var(--bg-primary));
    color: var(--text-faint);
    font: inherit;
    font-size: calc(var(--size-xs) * 0.85);
    cursor: pointer;
  }

  .diff-collapse-row:hover,
  .diff-collapse-row:focus-visible {
    color: var(--text-muted);
    outline: none;
  }

  .diff-row {
    display: grid;
    grid-template-columns: 3.5ch 3.5ch 1.5ch minmax(0, 1fr);
    min-width: max-content;
    color: var(--text-muted);
  }

  .diff-row.removed {
    background: color-mix(in srgb, var(--ui-danger) 10%, transparent);
  }

  .diff-row.added {
    background: color-mix(in srgb, var(--ui-success, var(--ui-accent)) 10%, transparent);
  }

  .line-number,
  .marker {
    user-select: none;
    color: var(--text-faint);
    text-align: right;
  }

  .line-number {
    padding: 0 6px;
    border-right: 1px solid var(--border-subtle);
  }

  .marker {
    padding: 0 6px;
  }

  .removed .marker,
  .removed .line-text {
    color: var(--ui-danger);
  }

  .added .marker,
  .added .line-text {
    color: var(--ui-success, var(--ui-accent));
  }

  .line-text {
    display: block;
    min-height: 1.5em;
    padding: 0 8px 0 0;
    color: inherit;
    white-space: pre-wrap;
  }

  .inline-diff :global(.char-highlight) {
    border-radius: 3px;
    background: color-mix(in srgb, currentColor 18%, transparent);
  }
</style>
