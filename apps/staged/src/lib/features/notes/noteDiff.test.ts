import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  computeNoteDiff,
  hunkTextRanges,
  type NoteDiffTextRange,
  type NoteDiffUnit,
} from './noteDiff';

function unit(text: string, kind = 'p', signature = ''): NoteDiffUnit {
  return { kind, text, signature };
}

const intro = unit('The cache is warmed when the app starts.');
const middle = unit('Entries are evicted oldest first, a hundred at a time.');
const outro = unit('Nothing is written to disk until the window closes.');

/** The parts of `text` the ranges cover. */
function covered(text: string, ranges: NoteDiffTextRange[]): string[] {
  return ranges.map(({ start, end }) => text.slice(start, end));
}

/** `count` distinct words, as a paragraph too long for `computeLineDiff` to compare. */
function longParagraph(prefix: string, count: number): string {
  return Array.from({ length: count }, (_, index) => `${prefix}${index}`).join(' ');
}

beforeEach(() => {
  // `computeLineDiff` reports every run on the console.
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

describe('computeNoteDiff', () => {
  it('finds nothing between identical versions', () => {
    const diff = computeNoteDiff([intro, middle, outro], [intro, middle, outro]);

    expect(diff.hunks).toEqual([]);
    expect(diff.ops).toEqual([
      { type: 'equal', previousIndex: 0, currentIndex: 0 },
      { type: 'equal', previousIndex: 1, currentIndex: 1 },
      { type: 'equal', previousIndex: 2, currentIndex: 2 },
    ]);
  });

  it('finds nothing between two empty versions', () => {
    expect(computeNoteDiff([], [])).toEqual({ ops: [], hunks: [] });
  });

  it('marks blocks only the current version has as added', () => {
    const diff = computeNoteDiff([intro, outro], [intro, middle, outro]);

    expect(diff.hunks).toEqual([
      {
        id: 'hunk-0',
        kind: 'added',
        ops: [{ type: 'added', currentIndex: 1 }],
        previousIndices: [],
        currentIndices: [1],
        previousStart: 1,
        currentStart: 1,
      },
    ]);
  });

  it('marks blocks only the previous version has as removed', () => {
    const diff = computeNoteDiff([intro, middle, outro], [intro, outro]);

    expect(diff.hunks).toEqual([
      {
        id: 'hunk-0',
        kind: 'removed',
        ops: [{ type: 'removed', previousIndex: 1 }],
        previousIndices: [1],
        currentIndices: [],
        previousStart: 1,
        currentStart: 1,
      },
    ]);
  });

  it('places a removal at the end of the note after the last block', () => {
    const [hunk] = computeNoteDiff([intro, middle, outro], [intro, middle]).hunks;

    expect(hunk.kind).toBe('removed');
    expect(hunk.currentStart).toBe(2);
  });

  it('pairs a reworded paragraph and finds the words that differ', () => {
    const reworded = unit('Entries are evicted newest first, a thousand at a time.');
    const diff = computeNoteDiff([intro, middle, outro], [intro, reworded, outro]);

    expect(diff.hunks).toHaveLength(1);
    const [hunk] = diff.hunks;
    expect(hunk.kind).toBe('modified');
    expect(hunk.previousIndices).toEqual([1]);
    expect(hunk.currentIndices).toEqual([1]);
    expect(hunk.ops).toHaveLength(1);
    const [op] = hunk.ops;
    if (op.type !== 'modified') throw new Error(`expected a modified op, got ${op.type}`);
    expect(op.wholeUnit).toBe(false);
    expect(covered(middle.text, op.previousRanges)).toEqual(['oldest', 'hundred']);
    expect(covered(reworded.text, op.currentRanges)).toEqual(['newest', 'thousand']);
  });

  it('marks a replaced paragraph as modified without pairing the two', () => {
    const replacement = unit('See the retention policy for how long logs are kept.');
    const diff = computeNoteDiff([intro, middle, outro], [intro, replacement, outro]);

    expect(diff.hunks).toHaveLength(1);
    const [hunk] = diff.hunks;
    expect(hunk.kind).toBe('modified');
    expect(hunk.ops).toEqual([
      { type: 'removed', previousIndex: 1 },
      { type: 'added', currentIndex: 1 },
    ]);
  });

  it('pairs a block whose signature alone changed, with no words to show', () => {
    const before = unit('Read the design doc first.', 'p', 'aaaa1111');
    const after = unit('Read the design doc first.', 'p', 'bbbb2222');
    const [hunk] = computeNoteDiff([intro, before, outro], [intro, after, outro]).hunks;

    expect(hunk.kind).toBe('modified');
    expect(hunk.ops).toEqual([
      {
        type: 'modified',
        previousIndex: 1,
        currentIndex: 1,
        previousRanges: [],
        currentRanges: [],
        wholeUnit: true,
      },
    ]);
  });

  it('pairs a block whose kind alone changed', () => {
    const before = unit('Eviction', 'h2');
    const after = unit('Eviction', 'h3');
    const [hunk] = computeNoteDiff([intro, before, outro], [intro, after, outro]).hunks;

    expect(hunk.ops).toMatchObject([{ type: 'modified', wholeUnit: true }]);
  });

  it('pairs a textless block whose signature changed, which shares too little to look similar', () => {
    const before = unit('', 'diagram', '0f0f0f0f');
    const after = unit('', 'diagram', 'c3c3c3c3');
    const [hunk] = computeNoteDiff([intro, before, outro], [intro, after, outro]).hunks;

    expect(hunk.kind).toBe('modified');
    expect(hunk.ops).toMatchObject([
      { type: 'modified', previousIndex: 1, currentIndex: 1, wholeUnit: true },
    ]);
  });

  it('does not pair textless blocks of different kinds', () => {
    const [hunk] = computeNoteDiff(
      [intro, unit('', 'media', '0f0f0f0f'), outro],
      [intro, unit('', 'diagram', 'c3c3c3c3'), outro]
    ).hunks;

    expect(hunk.kind).toBe('modified');
    expect(hunk.ops).toEqual([
      { type: 'removed', previousIndex: 1 },
      { type: 'added', currentIndex: 1 },
    ]);
  });

  it('keeps two changes apart when one unchanged block stands between them', () => {
    const added = unit('A second cache holds thumbnails.');
    const diff = computeNoteDiff([intro, middle, outro], [added, intro, middle]);

    expect(diff.hunks.map((hunk) => [hunk.id, hunk.kind])).toEqual([
      ['hunk-0', 'added'],
      ['hunk-1', 'removed'],
    ]);
    expect(diff.hunks[0]).toMatchObject({ previousStart: 0, currentStart: 0, currentIndices: [0] });
    expect(diff.hunks[1]).toMatchObject({
      previousStart: 2,
      currentStart: 3,
      previousIndices: [2],
    });
  });

  it('joins neighbouring changes into one hunk', () => {
    const reworded = unit('Entries are evicted newest first, a hundred at a time.');
    const added = unit('A second cache holds thumbnails.');
    const diff = computeNoteDiff([intro, middle, outro], [intro, reworded, added, outro]);

    expect(diff.hunks).toHaveLength(1);
    expect(diff.hunks[0]).toMatchObject({
      kind: 'modified',
      previousIndices: [1],
      currentIndices: [1, 2],
    });
    expect(diff.hunks[0].ops.map((op) => op.type)).toEqual(['modified', 'added']);
  });

  it('marks one changed list item and one changed table row, not their neighbours', () => {
    const previous = [
      unit('Install the toolchain', 'ol-li'),
      unit('Run the migration against a copy', 'ol-li'),
      unit('Restart the service', 'ol-li'),
      unit('Name Limit', 'tr'),
      unit('uploads 10 MB', 'tr'),
      unit('exports 50 MB', 'tr'),
    ];
    const current = [
      previous[0],
      unit('Run the migration against a fresh copy', 'ol-li'),
      previous[2],
      previous[3],
      previous[4],
      unit('exports 80 MB', 'tr'),
    ];
    const diff = computeNoteDiff(previous, current);

    expect(diff.hunks).toHaveLength(2);
    const [item, row] = diff.hunks;
    expect(item).toMatchObject({ kind: 'modified', previousIndices: [1], currentIndices: [1] });
    expect(row).toMatchObject({ kind: 'modified', previousIndices: [5], currentIndices: [5] });
    const [itemOp] = item.ops;
    const [rowOp] = row.ops;
    if (itemOp.type !== 'modified' || rowOp.type !== 'modified') {
      throw new Error('expected two modified ops');
    }
    expect(covered(current[1].text, itemOp.currentRanges).join('').trim()).toBe('fresh');
    expect(covered(previous[5].text, rowOp.previousRanges)).toEqual(['50']);
    expect(covered(current[5].text, rowOp.currentRanges)).toEqual(['80']);
  });

  it('shows a moved block as a removal and an addition', () => {
    const diff = computeNoteDiff([intro, middle, outro], [middle, outro, intro]);

    expect(diff.hunks.map((hunk) => hunk.kind)).toEqual(['removed', 'added']);
    expect(diff.hunks[0].previousIndices).toEqual([0]);
    expect(diff.hunks[1].currentIndices).toEqual([2]);
    expect(diff.ops.map((op) => op.type)).toEqual(['removed', 'equal', 'equal', 'added']);
  });

  it('shows a block that moved and was edited as a removal and an addition', () => {
    const edited = unit('The cache is warmed when the app first starts.');
    const diff = computeNoteDiff([intro, middle, outro], [middle, outro, edited]);

    expect(diff.hunks.map((hunk) => hunk.kind)).toEqual(['removed', 'added']);
  });

  describe('blocks too long for computeLineDiff to compare', () => {
    const before = unit(longParagraph('word', 220));
    const after = unit(before.text.replace('word7 ', 'changed7 ').replace(' word150', ' other150'));

    it('are long enough to be past its limit', () => {
      expect(before.text.length * after.text.length).toBeGreaterThan(1_000_000);
    });

    it('are still paired when most of their words are the same', () => {
      const [hunk] = computeNoteDiff([intro, before, outro], [intro, after, outro]).hunks;

      expect(hunk.kind).toBe('modified');
      expect(hunk.ops).toHaveLength(1);
      const [op] = hunk.ops;
      if (op.type !== 'modified') throw new Error(`expected a modified op, got ${op.type}`);
      expect(op.wholeUnit).toBe(false);
      expect(covered(before.text, op.previousRanges)).toEqual(['word7', 'word150']);
      expect(covered(after.text, op.currentRanges)).toEqual(['changed7', 'other150']);
    });

    it('stay a removal and an addition when they share no words', () => {
      const unrelated = unit(longParagraph('term', 220));
      const [hunk] = computeNoteDiff([intro, before, outro], [intro, unrelated, outro]).hunks;

      expect(hunk.kind).toBe('modified');
      expect(hunk.ops.map((op) => op.type)).toEqual(['removed', 'added']);
    });

    it('are not paired across kinds', () => {
      const asCode = { ...after, kind: 'code' };
      const [hunk] = computeNoteDiff([intro, before, outro], [intro, asCode, outro]).hunks;

      expect(hunk.ops.map((op) => op.type)).toEqual(['removed', 'added']);
    });

    it('keep the blocks around the pair in order', () => {
      const gone = unit('This sentence was dropped.');
      const fresh = unit('This heading is new', 'h2');
      const diff = computeNoteDiff([intro, gone, before, outro], [intro, fresh, after, outro]);

      expect(diff.hunks).toHaveLength(1);
      expect(diff.hunks[0].ops.map((op) => op.type)).toEqual(['removed', 'added', 'modified']);
      expect(diff.hunks[0].previousIndices).toEqual([1, 2]);
      expect(diff.hunks[0].currentIndices).toEqual([1, 2]);
    });
  });
});

describe('hunkTextRanges', () => {
  it('points out the changed words of a modified block on each side', () => {
    const reworded = unit('Entries are evicted newest first, a hundred at a time.');
    const [hunk] = computeNoteDiff([intro, middle, outro], [intro, reworded, outro]).hunks;

    const previous = hunkTextRanges(hunk, 'previous');
    const current = hunkTextRanges(hunk, 'current');

    expect(previous.map((entry) => entry.unitIndex)).toEqual([1]);
    expect(covered(middle.text, previous[0].ranges ?? [])).toEqual(['oldest']);
    expect(covered(reworded.text, current[0].ranges ?? [])).toEqual(['newest']);
  });

  it('leaves out the side of a modified block that only lost words', () => {
    const shorter = unit('Entries are evicted oldest first.');
    const [hunk] = computeNoteDiff([intro, middle, outro], [intro, shorter, outro]).hunks;

    expect(hunkTextRanges(hunk, 'previous')).toHaveLength(1);
    expect(hunkTextRanges(hunk, 'current')).toEqual([]);
  });

  it('points out the whole of a block that changed in signature alone', () => {
    const before = unit('Read the design doc first.', 'p', 'aaaa1111');
    const after = unit('Read the design doc first.', 'p', 'bbbb2222');
    const [hunk] = computeNoteDiff([intro, before, outro], [intro, after, outro]).hunks;

    expect(hunkTextRanges(hunk, 'previous')).toEqual([{ unitIndex: 1, ranges: null }]);
    expect(hunkTextRanges(hunk, 'current')).toEqual([{ unitIndex: 1, ranges: null }]);
  });

  it('points out each side of a replaced paragraph whole', () => {
    const replacement = unit('See the retention policy for how long logs are kept.');
    const [hunk] = computeNoteDiff([intro, middle, outro], [intro, replacement, outro]).hunks;

    expect(hunkTextRanges(hunk, 'previous')).toEqual([{ unitIndex: 1, ranges: null }]);
    expect(hunkTextRanges(hunk, 'current')).toEqual([{ unitIndex: 1, ranges: null }]);
  });

  it('points out nothing in a hunk of additions alone or removals alone', () => {
    const [added] = computeNoteDiff([intro, outro], [intro, middle, outro]).hunks;
    const [removed] = computeNoteDiff([intro, middle, outro], [intro, outro]).hunks;

    expect(hunkTextRanges(added, 'current')).toEqual([]);
    expect(hunkTextRanges(added, 'previous')).toEqual([]);
    expect(hunkTextRanges(removed, 'previous')).toEqual([]);
    expect(hunkTextRanges(removed, 'current')).toEqual([]);
  });
});
