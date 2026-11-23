/**
 * What changed between two versions of a note, as the note viewer shows it.
 *
 * The unit of comparison is a rendered block (a paragraph, a heading, a list
 * item, a table row, a figure), not a line of markdown: noteDiffUnits.ts
 * reads the blocks out of each version's rendered DOM, and this module only
 * sees them as `(kind, text, signature)` triples. Nothing here touches the
 * DOM.
 *
 * Each block becomes one string and the two versions go through
 * `computeLineDiff`, which aligns them and pairs similar blocks as modified,
 * with the words that differ. The result is read back as an ordered list of
 * ops and grouped into hunks: the runs of change between unchanged blocks. A
 * hunk is what the change bar marks and what a hover or click selects.
 */

import {
  computeCharHighlights,
  computeLineDiff,
  LCS_BAILOUT_PRODUCT,
  type CharHighlight,
} from '@builderbot/diff-viewer/utils';

/** One rendered block of a note version. */
export interface NoteDiffUnit {
  /** The block's tag name, or `media`, `diagram` or `code`. */
  kind: string;
  /** The text a reader sees in the block. */
  text: string;
  /**
   * What the block holds that its text does not show (link targets, the
   * image, a diagram's drawing), reduced to a short token: equal for equal
   * content, and otherwise meaningless.
   */
  signature: string;
}

/** Which version of the note something belongs to. */
export type NoteDiffSide = 'previous' | 'current';

/** A stretch of a unit's `text`: `[start, end)`. */
export interface NoteDiffTextRange {
  start: number;
  end: number;
}

export type NoteDiffOp =
  | { type: 'equal'; previousIndex: number; currentIndex: number }
  | {
      type: 'modified';
      previousIndex: number;
      currentIndex: number;
      /** The words of the previous text the current one dropped. */
      previousRanges: NoteDiffTextRange[];
      /** The words of the current text the previous one lacked. */
      currentRanges: NoteDiffTextRange[];
      /**
       * The text is the same on both sides: the block changed in kind or
       * signature only (a heading level, a link target, a diagram's drawing),
       * and there are no words to point at.
       */
      wholeUnit: boolean;
    }
  | { type: 'removed'; previousIndex: number }
  | { type: 'added'; currentIndex: number };

export type NoteDiffChange = Exclude<NoteDiffOp, { type: 'equal' }>;

/**
 * - `added`: only the current version has blocks here.
 * - `removed`: only the previous version has.
 * - `modified`: both have, whether as a pair of similar blocks or as a
 *   removal next to an addition.
 */
export type NoteDiffHunkKind = 'added' | 'removed' | 'modified';

/** A run of changed blocks with unchanged blocks, or the note's edge, on each side. */
export interface NoteDiffHunk {
  /** Names the hunk within its diff; not stable across diffs. */
  id: string;
  kind: NoteDiffHunkKind;
  ops: NoteDiffChange[];
  /** The previous version's units in the hunk, in order. */
  previousIndices: number[];
  /** The current version's units in the hunk, in order. */
  currentIndices: number[];
  /**
   * Where the hunk falls in the previous version: its first unit there, or,
   * when it has none, the unit that follows the gap (the unit count when the
   * gap is at the end).
   */
  previousStart: number;
  /** The same, in the current version. */
  currentStart: number;
}

export interface NoteDiff {
  /** Every unit of both versions, in order. */
  ops: NoteDiffOp[];
  hunks: NoteDiffHunk[];
}

/** A character no rendered text holds, so the parts of a line cannot run together. */
const FIELD_SEPARATOR = '\u0001';

function encodeUnit(unit: NoteDiffUnit): string {
  return unit.text + FIELD_SEPARATOR + unit.kind + FIELD_SEPARATOR + unit.signature;
}

/**
 * How much of two long blocks must be the same words for them to be one
 * block reworded: the bar `computeLineDiff` sets for the pairs it compares.
 */
const LONG_BLOCK_SIMILARITY = 0.55;

/**
 * How far past a removed block an added one is looked for as its other half,
 * as `computeLineDiff` looks ahead: a pairing costs a word diff, and a hunk
 * of many long blocks would otherwise try every combination.
 */
const MAX_PAIR_SCAN = 10;

export function computeNoteDiff(
  previous: readonly NoteDiffUnit[],
  current: readonly NoteDiffUnit[]
): NoteDiff {
  const previousLines = previous.map(encodeUnit);
  const currentLines = current.map(encodeUnit);
  const walked = walkOps(previous, current, computeLineDiff(previousLines, currentLines));
  const ops = pairLeftoverChanges(walked, previous, current, previousLines, currentLines);
  return { ops, hunks: groupHunks(ops) };
}

/**
 * The line diff's per-line classes as one ordered list, both versions
 * advancing together (the walk `buildDiffRows` does for tool-call diffs).
 *
 * `computeLineDiff` pairs unmatched lines in order without regard to the
 * unchanged lines between them, so a pair can sit on opposite sides of one: a
 * block that was edited and moved, or just moved. Such a pair cannot be one
 * op in an ordered list. Each half is emitted where it stands, as a removal
 * and an addition, without taking apart a later pair that stands together.
 */
function walkOps(
  previous: readonly NoteDiffUnit[],
  current: readonly NoteDiffUnit[],
  lineDiff: ReturnType<typeof computeLineDiff>
): NoteDiffOp[] {
  const pairs = new Map(lineDiff.modifiedPairs.map((pair) => [pair.beforeLineIndex, pair]));
  const pairsByAfter = new Map(lineDiff.modifiedPairs.map((pair) => [pair.afterLineIndex, pair]));
  const ops: NoteDiffOp[] = [];
  let p = 0;
  let c = 0;
  while (p < previous.length || c < current.length) {
    const before = p < previous.length ? lineDiff.beforeLines[p] : null;
    const after = c < current.length ? lineDiff.afterLines[c] : null;

    if (before === 'unchanged' && after === 'unchanged') {
      ops.push({ type: 'equal', previousIndex: p++, currentIndex: c++ });
      continue;
    }
    const pair = before === 'modified' ? pairs.get(p) : undefined;
    if (pair && pair.afterLineIndex === c) {
      ops.push(
        modifiedOp(
          p,
          c,
          clipToText(pair.beforeHighlights, previous[p].text.length),
          clipToText(pair.afterHighlights, current[c].text.length)
        )
      );
      p++;
      c++;
      continue;
    }
    if (before === 'removed') {
      ops.push({ type: 'removed', previousIndex: p++ });
      continue;
    }
    if (after === 'added') {
      ops.push({ type: 'added', currentIndex: c++ });
      continue;
    }
    // Half of a pair that straddles an unchanged unit. When both sides hold
    // one they are halves of two pairs, and pairs do not cross, so one of
    // them has its other half behind it already and stands alone. That one
    // goes first: emitting the other would take apart a pair whose halves
    // are about to meet.
    const afterPair = after === 'modified' ? pairsByAfter.get(c) : undefined;
    const afterIsOrphan = afterPair !== undefined && afterPair.beforeLineIndex < p;
    if (before === 'modified' && !afterIsOrphan) {
      ops.push({ type: 'removed', previousIndex: p++ });
      continue;
    }
    if (after === 'modified') {
      ops.push({ type: 'added', currentIndex: c++ });
      continue;
    }
    // Unchanged units pair up in order, so one version cannot run out while
    // the other still has one; advance all the same rather than spin.
    if (before !== null) p++;
    if (after !== null) c++;
  }
  return ops;
}

function modifiedOp(
  previousIndex: number,
  currentIndex: number,
  previousRanges: NoteDiffTextRange[],
  currentRanges: NoteDiffTextRange[]
): NoteDiffOp & { type: 'modified' } {
  return {
    type: 'modified',
    previousIndex,
    currentIndex,
    previousRanges,
    currentRanges,
    wholeUnit: previousRanges.length === 0 && currentRanges.length === 0,
  };
}

/**
 * A line's highlights, kept to its text. The line also carries the unit's
 * kind and signature after the text, and a difference there is not a word a
 * reader can be shown.
 */
function clipToText(highlights: CharHighlight[], textLength: number): NoteDiffTextRange[] {
  const ranges: NoteDiffTextRange[] = [];
  for (const { start, end } of highlights) {
    if (start < textLength) ranges.push({ start, end: Math.min(end, textLength) });
  }
  return ranges;
}

/**
 * Pair up blocks `computeLineDiff` left as a removal and an addition though
 * they are one block changed. Two cases reach here, both among the removals
 * and additions between the same two matched blocks:
 *
 * - Long blocks. `computeLineDiff` does not compare two lines whose lengths
 *   multiply past `LCS_BAILOUT_PRODUCT` (two paragraphs of a thousand
 *   characters), so a long paragraph with one word changed comes back as
 *   unrelated. Those pairs are compared by their words here instead, which
 *   is far fewer comparisons than by their characters.
 * - Blocks with the same text and a different signature. With little or no
 *   text (an image, a diagram) the signature is most of the line, and two
 *   different ones do not look similar.
 *
 * Only blocks of one kind are paired, in order, so the pairs cannot cross
 * each other or the pairs already made.
 */
function pairLeftoverChanges(
  ops: NoteDiffOp[],
  previous: readonly NoteDiffUnit[],
  current: readonly NoteDiffUnit[],
  previousLines: string[],
  currentLines: string[]
): NoteDiffOp[] {
  const result: NoteDiffOp[] = [];
  let run: NoteDiffOp[] = [];

  const flush = () => {
    if (run.length === 0) return;
    const removed = run.filter((op) => op.type === 'removed');
    const added = run.filter((op) => op.type === 'added');
    const pairs: (NoteDiffOp & { type: 'modified' })[] = [];
    let from = 0;
    for (const { previousIndex } of removed) {
      const limit = Math.min(from + MAX_PAIR_SCAN, added.length);
      for (let at = from; at < limit; at++) {
        const { currentIndex } = added[at];
        const ranges = pairedRanges(
          previous[previousIndex],
          current[currentIndex],
          previousLines[previousIndex],
          currentLines[currentIndex]
        );
        if (!ranges) continue;
        pairs.push(modifiedOp(previousIndex, currentIndex, ranges.previous, ranges.current));
        from = at + 1;
        break;
      }
    }

    if (pairs.length === 0) {
      result.push(...run);
    } else {
      let r = 0;
      let a = 0;
      for (const pair of pairs) {
        while (removed[r].previousIndex < pair.previousIndex) result.push(removed[r++]);
        while (added[a].currentIndex < pair.currentIndex) result.push(added[a++]);
        result.push(pair);
        r++;
        a++;
      }
      result.push(...removed.slice(r), ...added.slice(a));
    }
    run = [];
  };

  for (const op of ops) {
    if (op.type === 'removed' || op.type === 'added') {
      run.push(op);
    } else {
      flush();
      result.push(op);
    }
  }
  flush();
  return result;
}

/** The word ranges of a removal and an addition that are one block changed, or `null`. */
function pairedRanges(
  previous: NoteDiffUnit,
  current: NoteDiffUnit,
  previousLine: string,
  currentLine: string
): { previous: NoteDiffTextRange[]; current: NoteDiffTextRange[] } | null {
  if (previous.kind !== current.kind) return null;
  if (previous.text === current.text) {
    // The same line twice is a block that moved, not one that changed.
    return previous.signature === current.signature ? null : { previous: [], current: [] };
  }

  // A pair `computeLineDiff` could compare, it has: they are not similar.
  if (previousLine.trim().length * currentLine.trim().length <= LCS_BAILOUT_PRODUCT) return null;
  // Texts too far apart in length cannot share enough to pass, whatever they hold.
  const shorter = Math.min(previous.text.length, current.text.length);
  const total = previous.text.length + current.text.length;
  if ((2 * shorter) / total <= LONG_BLOCK_SIMILARITY) return null;

  const { beforeHighlights, afterHighlights } = computeCharHighlights(previous.text, current.text);
  const changed = coveredLength(beforeHighlights) + coveredLength(afterHighlights);
  if (1 - changed / total <= LONG_BLOCK_SIMILARITY) return null;
  return { previous: beforeHighlights, current: afterHighlights };
}

function coveredLength(ranges: NoteDiffTextRange[]): number {
  return ranges.reduce((length, range) => length + range.end - range.start, 0);
}

function groupHunks(ops: NoteDiffOp[]): NoteDiffHunk[] {
  const hunks: NoteDiffHunk[] = [];
  let open: NoteDiffHunk | null = null;
  // Units of each version already passed.
  let p = 0;
  let c = 0;
  for (const op of ops) {
    if (op.type === 'equal') {
      open = null;
      p++;
      c++;
      continue;
    }
    if (!open) {
      open = {
        id: `hunk-${hunks.length}`,
        kind: 'modified',
        ops: [],
        previousIndices: [],
        currentIndices: [],
        previousStart: p,
        currentStart: c,
      };
      hunks.push(open);
    }
    open.ops.push(op);
    if (op.type !== 'added') {
      open.previousIndices.push(op.previousIndex);
      p++;
    }
    if (op.type !== 'removed') {
      open.currentIndices.push(op.currentIndex);
      c++;
    }
  }
  for (const hunk of hunks) {
    if (hunk.previousIndices.length === 0) hunk.kind = 'added';
    else if (hunk.currentIndices.length === 0) hunk.kind = 'removed';
  }
  return hunks;
}

/** The text of one unit to point out within a hunk. */
export interface NoteDiffUnitRanges {
  /** The unit's index in its version. */
  unitIndex: number;
  /** Stretches of the unit's text, or `null` for the whole unit. */
  ranges: NoteDiffTextRange[] | null;
}

/**
 * What to point out, on one side, beyond the hunk's own colour.
 *
 * In a block that was modified it is the words that differ, or the whole
 * block when none do. In a hunk that holds both versions' blocks it is also
 * every block only this side has: within a change marked as a modification,
 * that is the part which is wholly new, or wholly gone. A hunk of additions
 * alone, or removals alone, is that already, and has nothing to point out.
 */
export function hunkTextRanges(hunk: NoteDiffHunk, side: NoteDiffSide): NoteDiffUnitRanges[] {
  const result: NoteDiffUnitRanges[] = [];
  for (const op of hunk.ops) {
    if (op.type === 'modified') {
      const unitIndex = side === 'previous' ? op.previousIndex : op.currentIndex;
      const ranges = side === 'previous' ? op.previousRanges : op.currentRanges;
      if (op.wholeUnit) result.push({ unitIndex, ranges: null });
      else if (ranges.length > 0) result.push({ unitIndex, ranges });
    } else if (hunk.kind === 'modified') {
      if (op.type === 'removed' && side === 'previous') {
        result.push({ unitIndex: op.previousIndex, ranges: null });
      } else if (op.type === 'added' && side === 'current') {
        result.push({ unitIndex: op.currentIndex, ranges: null });
      }
    }
  }
  return result;
}
