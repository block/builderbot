import { computeLineDiff, type CharHighlight } from '@builderbot/diff-viewer/utils';
import type { ToolCallDiff } from '../toolCallViewModel';

export type DiffTone = 'context' | 'removed' | 'added' | 'modified-removed' | 'modified-added';

export interface DiffRow {
  key: string;
  oldLine: number | null;
  newLine: number | null;
  marker: ' ' | '-' | '+';
  text: string;
  tone: DiffTone;
  highlights: CharHighlight[];
}

export type DiffRowBlock =
  | { kind: 'visible'; key: string; rows: DiffRow[] }
  | { kind: 'collapsed'; key: string; rows: DiffRow[] };

export interface DiffRowStats {
  added: number;
  removed: number;
}

export interface GroupDiffRowsOptions {
  context?: number;
  minHidden?: number;
}

export function buildDiffRows(diff: ToolCallDiff): DiffRow[] {
  const beforeLines = splitLines(diff.oldText);
  const afterLines = splitLines(diff.newText);
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

    if (beforeClass === 'removed') {
      result.push({
        key: `removed:${beforeIndex}`,
        oldLine: beforeIndex + 1,
        newLine: null,
        marker: '-',
        text: beforeLines[beforeIndex],
        tone: 'removed',
        highlights: [],
      });
      beforeIndex += 1;
      continue;
    }

    const afterPair = modifiedByAfter.get(afterIndex);
    if (afterClass === 'added') {
      result.push({
        key: `added:${afterIndex}`,
        oldLine: null,
        newLine: afterIndex + 1,
        marker: '+',
        text: afterLines[afterIndex],
        tone: 'added',
        highlights: [],
      });
      afterIndex += 1;
      continue;
    }

    // A modified pair can straddle an LCS anchor (e.g. a line edited while
    // moving across an unchanged line), so it never aligns for the paired
    // branch above. Render each half as its own row instead of dropping it.
    if (beforeIndex < beforeLines.length && beforeClass !== 'unchanged') {
      result.push({
        key: `removed:${beforeIndex}`,
        oldLine: beforeIndex + 1,
        newLine: null,
        marker: '-',
        text: beforeLines[beforeIndex],
        tone: 'modified-removed',
        highlights: modifiedPair?.beforeHighlights ?? [],
      });
      beforeIndex += 1;
      continue;
    }

    if (afterIndex < afterLines.length && afterClass !== 'unchanged') {
      result.push({
        key: `added:${afterIndex}`,
        oldLine: null,
        newLine: afterIndex + 1,
        marker: '+',
        text: afterLines[afterIndex],
        tone: 'modified-added',
        highlights: afterPair?.afterHighlights ?? [],
      });
      afterIndex += 1;
      continue;
    }

    // Unreachable for well-formed line diffs; advance to guarantee progress.
    if (beforeIndex < beforeLines.length) {
      beforeIndex += 1;
    }
    if (afterIndex < afterLines.length) {
      afterIndex += 1;
    }
  }

  return result;
}

export function diffRowStats(rows: DiffRow[]): DiffRowStats {
  let added = 0;
  let removed = 0;
  for (const row of rows) {
    if (row.marker === '+') added += 1;
    else if (row.marker === '-') removed += 1;
  }
  return { added, removed };
}

export function groupDiffRows(rows: DiffRow[], options: GroupDiffRowsOptions = {}): DiffRowBlock[] {
  const context = options.context ?? 3;
  const minHidden = options.minHidden ?? 3;

  const segments: { isContext: boolean; rows: DiffRow[] }[] = [];
  for (const row of rows) {
    const isContext = row.tone === 'context';
    const last = segments[segments.length - 1];
    if (last && last.isContext === isContext) {
      last.rows.push(row);
    } else {
      segments.push({ isContext, rows: [row] });
    }
  }

  // A diff with no changed rows has nothing to anchor context around.
  if (!segments.some((segment) => !segment.isContext)) {
    return rows.length > 0 ? [{ kind: 'visible', key: `visible:${rows[0].key}`, rows }] : [];
  }

  const blocks: DiffRowBlock[] = [];
  const pushVisible = (visibleRows: DiffRow[]) => {
    if (visibleRows.length === 0) return;
    const last = blocks[blocks.length - 1];
    if (last?.kind === 'visible') {
      last.rows = [...last.rows, ...visibleRows];
    } else {
      blocks.push({ kind: 'visible', key: `visible:${visibleRows[0].key}`, rows: visibleRows });
    }
  };

  segments.forEach((segment, index) => {
    if (!segment.isContext) {
      pushVisible(segment.rows);
      return;
    }

    // Leading runs only pad the change below them, trailing runs the one above.
    const head = index === 0 ? 0 : context;
    const tail = index === segments.length - 1 ? 0 : context;
    const hidden = segment.rows.length - head - tail;
    if (hidden < minHidden) {
      pushVisible(segment.rows);
      return;
    }

    pushVisible(segment.rows.slice(0, head));
    const hiddenRows = segment.rows.slice(head, head + hidden);
    blocks.push({ kind: 'collapsed', key: `collapsed:${hiddenRows[0].key}`, rows: hiddenRows });
    pushVisible(segment.rows.slice(head + hidden));
  });

  return blocks;
}

function splitLines(text: string | null): string[] {
  if (text === null || text === '') return [];
  return text.split('\n');
}
