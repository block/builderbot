import { describe, expect, it } from 'vitest';
import type { ToolCallDiff } from '../toolCallViewModel';
import { buildDiffRows, diffRowStats, groupDiffRows, type DiffRow } from './inlineDiffRows';

function diff(oldText: string | null, newText: string | null): ToolCallDiff {
  return {
    path: 'src/example.ts',
    oldText,
    newText,
    kind: oldText === null ? 'created' : newText === null ? 'deleted' : 'modified',
  };
}

function lines(count: number, prefix = 'line'): string {
  return Array.from({ length: count }, (_, index) => `${prefix} ${index + 1}`).join('\n');
}

describe('buildDiffRows', () => {
  it('pairs modified lines as removed and added rows with line numbers', () => {
    const rows = buildDiffRows(diff('const a = 1;\nshared\n', 'const a = 2;\nshared\n'));

    expect(rows[0]).toMatchObject({
      marker: '-',
      tone: 'modified-removed',
      oldLine: 1,
      newLine: null,
      text: 'const a = 1;',
    });
    expect(rows[1]).toMatchObject({
      marker: '+',
      tone: 'modified-added',
      oldLine: null,
      newLine: 1,
      text: 'const a = 2;',
    });
    expect(rows[2]).toMatchObject({ marker: ' ', tone: 'context', oldLine: 2, newLine: 2 });
  });

  it('marks every line added for created files', () => {
    const rows = buildDiffRows(diff(null, 'first\nsecond'));

    expect(rows).toHaveLength(2);
    expect(rows.every((row) => row.marker === '+' && row.oldLine === null)).toBe(true);
  });
});

describe('diffRowStats', () => {
  it('counts added and removed rows', () => {
    const rows = buildDiffRows(diff('keep\nold 1\nold 2\n', 'keep\nnew 1\n'));

    expect(diffRowStats(rows)).toEqual({ added: 1, removed: 2 });
  });
});

describe('groupDiffRows', () => {
  function toneSummary(blocks: ReturnType<typeof groupDiffRows>): string[] {
    return blocks.map((block) =>
      block.kind === 'collapsed' ? `collapsed:${block.rows.length}` : `visible:${block.rows.length}`
    );
  }

  it('collapses a long unchanged run between changes, keeping context on both sides', () => {
    const middle = lines(12, 'same');
    const rows = buildDiffRows(
      diff(`start old\n${middle}\nend old`, `start new\n${middle}\nend new`)
    );

    // 2 change rows, 3 context, 6 collapsed, 3 context, 2 change rows.
    expect(toneSummary(groupDiffRows(rows))).toEqual(['visible:5', 'collapsed:6', 'visible:5']);
  });

  it('trims leading context to the rows adjacent to the first change', () => {
    const rows = buildDiffRows(diff(`${lines(10)}\nold tail`, `${lines(10)}\nnew tail`));

    const blocks = groupDiffRows(rows);
    expect(toneSummary(blocks)).toEqual(['collapsed:7', 'visible:5']);
    expect(blocks[1].rows.map((row) => row.marker)).toEqual([' ', ' ', ' ', '-', '+']);
  });

  it('trims trailing context to the rows adjacent to the last change', () => {
    const rows = buildDiffRows(diff(`old head\n${lines(10)}`, `new head\n${lines(10)}`));

    expect(toneSummary(groupDiffRows(rows))).toEqual(['visible:5', 'collapsed:7']);
  });

  it('keeps short unchanged runs visible instead of collapsing them', () => {
    const rows = buildDiffRows(
      diff(`old\n${lines(8, 'same')}\nold end`, `new\n${lines(8, 'same')}\nnew end`)
    );

    // 8 context rows minus 3+3 kept leaves 2 hidden, below the collapse threshold.
    expect(toneSummary(groupDiffRows(rows))).toEqual(['visible:12']);
  });

  it('keeps a fully unchanged diff visible as one block', () => {
    const rows: DiffRow[] = buildDiffRows(diff(lines(10), lines(10)));

    expect(toneSummary(groupDiffRows(rows))).toEqual(['visible:10']);
  });

  it('returns no blocks for empty diffs', () => {
    expect(groupDiffRows([])).toEqual([]);
  });
});
