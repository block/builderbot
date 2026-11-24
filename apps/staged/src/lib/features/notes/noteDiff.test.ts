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

/**
 * A paragraph of `length` characters in six-letter words of `letters`,
 * numbered so that no two are the same. Two paragraphs in letters of their
 * own share the spaces between their words and nothing else.
 */
function prose(letters: string, index: number, length: number): string {
  let text = `${index} `;
  for (let word = 0; text.length < length; word++) {
    const start = (index * 7 + word * 3) % letters.length;
    text += `${(letters + letters).slice(start, start + 6)} `;
  }
  return text.slice(0, length).trim();
}

const EARLY_LETTERS = 'abcdefgh';
const MIDDLE_LETTERS = 'ijklmnopq';
const LATE_LETTERS = 'rstuvwxyz';

/** `count` paragraphs of `length` characters, and the same number that share nothing with them. */
function rewritten(count: number, length: number): [NoteDiffUnit[], NoteDiffUnit[]] {
  return [
    Array.from({ length: count }, (_, index) => unit(prose(EARLY_LETTERS, index, length))),
    Array.from({ length: count }, (_, index) => unit(prose(LATE_LETTERS, index, length))),
  ];
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
    expect(computeNoteDiff([], [])).toEqual({ ops: [], hunks: [], capped: false });
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

  it('gives a reworded phrase as one stretch of words, without the spaces around it', () => {
    // The word diff compares the spaces between words too. It matches some
    // and not others, which left a phrase in pieces with a space on its end.
    const before = unit(
      'A failed upload is never retried. The client gives up on the first error.'
    );
    const after = unit(
      'A failed upload is retried only by hand. The client gives up on the first error.'
    );
    const diff = computeNoteDiff([intro, before, outro], [intro, after, outro]);

    const [op] = diff.hunks[0].ops;
    if (op.type !== 'modified') throw new Error(`expected a modified op, got ${op.type}`);
    expect(covered(before.text, op.previousRanges)).toEqual(['never retried']);
    expect(covered(after.text, op.currentRanges)).toEqual(['retried only by hand']);
  });

  it('marks a block whole when only the spacing of its text changed', () => {
    const before = unit('if (ready) {\n  start();\n}', 'code');
    const after = unit('if (ready) {\n    start();\n}', 'code');
    const diff = computeNoteDiff([intro, before, outro], [intro, after, outro]);

    expect(diff.hunks).toHaveLength(1);
    const [op] = diff.hunks[0].ops;
    if (op.type !== 'modified') throw new Error(`expected a modified op, got ${op.type}`);
    expect(op.previousRanges).toEqual([]);
    expect(op.currentRanges).toEqual([]);
    expect(op.wholeUnit).toBe(true);
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

  it('keeps a reworded block paired when the block before it moved and was edited', () => {
    // The edited intro now stands after `middle`. `computeLineDiff` pairs it
    // with the old one across `middle`, and the outro with its rewording.
    const editedIntro = unit('The cache is warmed when the app first starts.');
    const rewordedOutro = unit('Nothing is written to disk until the app closes.');
    const diff = computeNoteDiff([intro, middle, outro], [middle, editedIntro, rewordedOutro]);

    // The moved block's two halves stand where they are, and the reworded
    // one stays a pair: the half that has lost its other half goes first.
    expect(diff.ops.map((op) => op.type)).toEqual(['removed', 'equal', 'added', 'modified']);
    const reworded = diff.ops[3];
    if (reworded.type !== 'modified')
      throw new Error(`expected a modified op, got ${reworded.type}`);
    expect([reworded.previousIndex, reworded.currentIndex]).toEqual([2, 2]);
    expect(covered(outro.text, reworded.previousRanges)).toEqual(['window']);
    expect(covered(rewordedOutro.text, reworded.currentRanges)).toEqual(['app']);
    expect(diff.hunks.map((hunk) => hunk.kind)).toEqual(['removed', 'modified']);
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

describe('the work budget', () => {
  it('is not reached by a note that was rewritten end to end, when the note is short', () => {
    const [previous, current] = rewritten(20, 150);

    const diff = computeNoteDiff(previous, current);

    expect(diff.capped).toBe(false);
    expect(diff.hunks).toHaveLength(1);
  });

  it('stops comparing the blocks of a long note that was rewritten end to end', () => {
    // 120 blocks, each compared with up to eleven others at 160,000 cells a
    // time: some 200 million cells, uncapped.
    const [previous, current] = rewritten(120, 400);

    const diff = computeNoteDiff(previous, current);

    expect(diff.capped).toBe(true);
    // The change reads as replaced: every block of both versions, in one
    // hunk that is neither an addition nor a removal.
    expect(diff.hunks).toHaveLength(1);
    const [hunk] = diff.hunks;
    expect(hunk.kind).toBe('modified');
    expect(hunk.previousIndices).toHaveLength(120);
    expect(hunk.currentIndices).toHaveLength(120);
    expect(new Set(hunk.ops.map((op) => op.type))).toEqual(new Set(['removed', 'added']));
    // Removed on one side and added on the other, with no words to point out.
    expect(hunkTextRanges(hunk, 'previous')).toEqual([]);
    expect(hunkTextRanges(hunk, 'current')).toEqual([]);
  });

  it('leaves a reworded block unpaired when there is no room to compare it', () => {
    const reworded = unit(middle.text.replace('oldest', 'newest'));
    expect(computeNoteDiff([intro, middle], [intro, reworded]).hunks[0].ops[0].type).toBe(
      'modified'
    );

    const diff = computeNoteDiff([intro, middle], [intro, reworded], 100);

    expect(diff.capped).toBe(true);
    expect(diff.hunks).toHaveLength(1);
    expect(diff.hunks[0].kind).toBe('modified');
    expect(diff.hunks[0].ops).toEqual([
      { type: 'removed', previousIndex: 1 },
      { type: 'added', currentIndex: 1 },
    ]);
  });

  it('leaves long blocks unpaired when there is no room to compare their words', () => {
    const before = unit(longParagraph('word', 220));
    const after = unit(before.text.replace('word7 ', 'changed7 '));
    // 439 words and spaces a side: 192,721 cells.
    const diff = computeNoteDiff([intro, before], [intro, after], 1_000_000);

    expect(diff.capped).toBe(true);
    expect(diff.hunks[0].ops.map((op) => op.type)).toEqual(['removed', 'added']);
  });

  it('still pairs blocks whose signature alone changed, which costs nothing', () => {
    const diagram = unit('', 'diagram', 'drawing-1');
    const redrawn = unit('', 'diagram', 'drawing-2');

    const diff = computeNoteDiff([intro, diagram], [intro, redrawn], 0);

    expect(diff.hunks[0].ops.map((op) => op.type)).toEqual(['modified']);
  });

  it('keeps the changed words of small changes among rewritten blocks', () => {
    // Every other block rewritten, at some four million cells each, and
    // every other one with a word changed.
    const [gone, fresh] = rewritten(40, 600);
    const edited = gone.map((_, index) => unit(prose(MIDDLE_LETTERS, index, 600)));
    const reworded = edited.map((block) => unit(block.text.replace(/ \w+ /, ' changed ')));

    const diff = computeNoteDiff(
      gone.flatMap((block, index) => [block, edited[index]]),
      fresh.flatMap((block, index) => [block, reworded[index]])
    );

    // The rewritten blocks alone are far past the budget.
    expect(diff.capped).toBe(true);
    const pairs = diff.ops.filter((op) => op.type === 'modified');
    expect(pairs.map((op) => op.previousIndex)).toEqual(gone.map((_, index) => 2 * index + 1));
    expect(
      pairs.every(
        (op, index) => covered(reworded[index].text, op.currentRanges).join() === 'changed'
      )
    ).toBe(true);
  });

  it('is not reached by one word changed in every block of a long note', () => {
    const previous = Array.from({ length: 200 }, (_, index) =>
      unit(prose(MIDDLE_LETTERS, index, 600))
    );
    const current = previous.map((block) => unit(block.text.replace(/ \w+ /, ' changed ')));

    const diff = computeNoteDiff(previous, current);

    expect(diff.capped).toBe(false);
    expect(diff.ops.every((op) => op.type === 'modified')).toBe(true);
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

  it('points out nothing in a replaced paragraph: each side is a whole change', () => {
    const replacement = unit('See the retention policy for how long logs are kept.');
    const [hunk] = computeNoteDiff([intro, middle, outro], [intro, replacement, outro]).hunks;

    expect(hunk.kind).toBe('modified');
    expect(hunk.ops.map((op) => op.type)).toEqual(['removed', 'added']);
    expect(hunkTextRanges(hunk, 'previous')).toEqual([]);
    expect(hunkTextRanges(hunk, 'current')).toEqual([]);
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
