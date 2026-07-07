<script lang="ts">
  import { computeLineDiff, type CharHighlight } from '@builderbot/diff-viewer/utils';
  import type { ToolCallDiff } from '../toolCallViewModel';

  type DiffTone = 'context' | 'removed' | 'added' | 'modified-removed' | 'modified-added';

  interface DiffRow {
    key: string;
    oldLine: number | null;
    newLine: number | null;
    marker: ' ' | '-' | '+';
    text: string;
    tone: DiffTone;
    highlights: CharHighlight[];
  }

  interface Segment {
    text: string;
    highlighted: boolean;
  }

  interface Props {
    diff: ToolCallDiff;
  }

  let { diff }: Props = $props();
  let rows = $derived.by(() => buildRows(diff));

  function splitLines(text: string | null): string[] {
    if (text === null || text === '') return [];
    return text.split('\n');
  }

  function buildRows(toolDiff: ToolCallDiff): DiffRow[] {
    const beforeLines = splitLines(toolDiff.oldText);
    const afterLines = splitLines(toolDiff.newText);
    const lineDiff = computeLineDiff(beforeLines, afterLines);
    const modifiedByBefore = new Map(
      lineDiff.modifiedPairs.map((pair) => [pair.beforeLineIndex, pair])
    );
    const modifiedByAfter = new Map(
      lineDiff.modifiedPairs.map((pair) => [pair.afterLineIndex, pair])
    );
    const result: DiffRow[] = [];
    let beforeIndex = 0;
    let afterIndex = 0;

    while (beforeIndex < beforeLines.length || afterIndex < afterLines.length) {
      const beforeClass = lineDiff.beforeLines[beforeIndex];
      const afterClass = lineDiff.afterLines[afterIndex];

      if (
        beforeIndex < beforeLines.length &&
        afterIndex < afterLines.length &&
        beforeClass === 'unchanged' &&
        afterClass === 'unchanged'
      ) {
        result.push({
          key: `context:${beforeIndex}:${afterIndex}`,
          oldLine: beforeIndex + 1,
          newLine: afterIndex + 1,
          marker: ' ',
          text: beforeLines[beforeIndex],
          tone: 'context',
          highlights: [],
        });
        beforeIndex += 1;
        afterIndex += 1;
        continue;
      }

      const modifiedPair = modifiedByBefore.get(beforeIndex);
      if (modifiedPair && modifiedPair.afterLineIndex === afterIndex) {
        result.push({
          key: `mod-before:${beforeIndex}`,
          oldLine: beforeIndex + 1,
          newLine: null,
          marker: '-',
          text: beforeLines[beforeIndex],
          tone: 'modified-removed',
          highlights: modifiedPair.beforeHighlights,
        });
        result.push({
          key: `mod-after:${afterIndex}`,
          oldLine: null,
          newLine: afterIndex + 1,
          marker: '+',
          text: afterLines[afterIndex],
          tone: 'modified-added',
          highlights: modifiedPair.afterHighlights,
        });
        beforeIndex += 1;
        afterIndex += 1;
        continue;
      }

      if (beforeClass === 'removed' || (beforeClass === 'modified' && !modifiedPair)) {
        result.push({
          key: `removed:${beforeIndex}`,
          oldLine: beforeIndex + 1,
          newLine: null,
          marker: '-',
          text: beforeLines[beforeIndex],
          tone: beforeClass === 'modified' ? 'modified-removed' : 'removed',
          highlights: modifiedByBefore.get(beforeIndex)?.beforeHighlights ?? [],
        });
        beforeIndex += 1;
        continue;
      }

      const afterPair = modifiedByAfter.get(afterIndex);
      if (afterClass === 'added' || (afterClass === 'modified' && !afterPair)) {
        result.push({
          key: `added:${afterIndex}`,
          oldLine: null,
          newLine: afterIndex + 1,
          marker: '+',
          text: afterLines[afterIndex],
          tone: afterClass === 'modified' ? 'modified-added' : 'added',
          highlights: modifiedByAfter.get(afterIndex)?.afterHighlights ?? [],
        });
        afterIndex += 1;
        continue;
      }

      if (beforeIndex < beforeLines.length) {
        beforeIndex += 1;
      }
      if (afterIndex < afterLines.length) {
        afterIndex += 1;
      }
    }

    return result;
  }

  function lineNumber(value: number | null): string {
    return value === null ? '' : String(value);
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

<div class="inline-diff" aria-label={`${diff.path} diff`}>
  {#if rows.length === 0}
    <div class="inline-diff-empty">No changes</div>
  {:else}
    {#each rows as row (row.key)}
      <div
        class="diff-row"
        class:removed={row.tone === 'removed' || row.tone === 'modified-removed'}
        class:added={row.tone === 'added' || row.tone === 'modified-added'}
        class:modified={row.tone === 'modified-removed' || row.tone === 'modified-added'}
      >
        <span class="line-number old">{lineNumber(row.oldLine)}</span>
        <span class="line-number new">{lineNumber(row.newLine)}</span>
        <span class="marker">{row.marker}</span>
        <code class="line-text">{@html highlightedLineHtml(row.text, row.highlights)}</code>
      </div>
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

  .inline-diff-empty {
    padding: 8px 10px;
    color: var(--text-faint);
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
