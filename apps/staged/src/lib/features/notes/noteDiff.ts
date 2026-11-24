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
  charHighlightsWork,
  computeCharHighlights,
  computeLineDiff,
  DiffWorkBudget,
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
       * There are no words to point at: the block changed in kind or
       * signature only (a heading level, a link target, a diagram's drawing),
       * or in the spacing of its text alone (a code block indented again).
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
  /**
   * The work budget refused a comparison (see `NOTE_DIFF_WORK_BUDGET`): some
   * removals and additions might be one block changed.
   */
  capped: boolean;
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

/**
 * How much work goes into finding the blocks a revision reworded, and the
 * words that changed in them: twelve million cells of a `DiffWorkBudget`,
 * about 50 ms of comparing.
 *
 * Without a cap that work grows with the number of changed blocks times the
 * square of their length, and the diff is computed while the note opens, on
 * the thread that draws it: a long note rewritten end to end took seconds.
 *
 * `computeLineDiff` spends the budget in note order, and the long blocks
 * paired here spend what it leaves. Blocks that differ by a few words cost
 * little, however long they are and however many. The budget goes on blocks
 * with little in common, which are the ones with no changed words to show,
 * and its tail is kept from them, so that rewritten blocks do not take the
 * words out of the small changes among them.
 *
 * Blocks whose comparison was refused are left as removals and additions.
 * Their change is still marked, and as modified where it holds blocks of
 * both versions: it reads as replaced, the previous version's blocks removed
 * and the note's added, with no changed words, and the two sides lined up by
 * the change instead of block by block. Blocks with the same text and a different signature (an image, a
 * diagram) are paired regardless, which costs nothing.
 */
export const NOTE_DIFF_WORK_BUDGET = 12_000_000;

export function computeNoteDiff(
  previous: readonly NoteDiffUnit[],
  current: readonly NoteDiffUnit[],
  workBudget: number = NOTE_DIFF_WORK_BUDGET
): NoteDiff {
  const previousLines = previous.map(encodeUnit);
  const currentLines = current.map(encodeUnit);
  const budget = new DiffWorkBudget(workBudget);
  const lineDiff = computeLineDiff(previousLines, currentLines, { budget });
  const walked = walkOps(previous, current, lineDiff);
  const ops = pairLeftoverChanges(walked, previous, current, previousLines, currentLines, budget);
  return { ops, hunks: groupHunks(ops), capped: budget.capped };
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
          wordRanges(previous[p].text, pair.beforeHighlights),
          wordRanges(current[c].text, pair.afterHighlights)
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
 * A line's highlights as the words of its unit's text a reader is shown.
 *
 * Kept to the text: the line also carries the unit's kind and signature
 * after it, and a difference there is not a word.
 *
 * And made whole. The word diff compares the spaces between words as it
 * does the words, so of a reworded phrase it matches some spaces and not
 * others: the phrase comes back in pieces, some with a space on the end.
 * Here a stretch loses the spaces at its ends, and stretches with nothing
 * but spaces between them are one. A highlight that was only spaces (a code
 * block indented again) is no stretch at all.
 */
function wordRanges(text: string, highlights: readonly CharHighlight[]): NoteDiffTextRange[] {
  const ranges: NoteDiffTextRange[] = [];
  for (const highlight of highlights) {
    let start = Math.min(highlight.start, text.length);
    let end = Math.min(highlight.end, text.length);
    while (start < end && isSpace(text[start])) start++;
    while (end > start && isSpace(text[end - 1])) end--;
    if (start >= end) continue;
    const last = ranges[ranges.length - 1];
    if (last && onlySpaces(text, last.end, start)) last.end = end;
    else ranges.push({ start, end });
  }
  return ranges;
}

function isSpace(character: string): boolean {
  return /\s/.test(character);
}

function onlySpaces(text: string, start: number, end: number): boolean {
  for (let i = start; i < end; i++) {
    if (!isSpace(text[i])) return false;
  }
  return true;
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
  currentLines: string[],
  budget: DiffWorkBudget
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
          currentLines[currentIndex],
          budget
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

/**
 * The word ranges of a removal and an addition that are one block changed,
 * or `null`. Comparing two long blocks by word is paid for from `budget`,
 * and two it has no room for are not paired.
 */
function pairedRanges(
  previous: NoteDiffUnit,
  current: NoteDiffUnit,
  previousLine: string,
  currentLine: string,
  budget: DiffWorkBudget
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

  if (!budget.spend(charHighlightsWork(previous.text, current.text))) return null;
  const { beforeHighlights, afterHighlights } = computeCharHighlights(previous.text, current.text);
  const changed = coveredLength(beforeHighlights) + coveredLength(afterHighlights);
  if (1 - changed / total <= LONG_BLOCK_SIMILARITY) return null;
  return {
    previous: wordRanges(previous.text, beforeHighlights),
    current: wordRanges(current.text, afterHighlights),
  };
}

function coveredLength(ranges: readonly CharHighlight[]): number {
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
 * The words to point out on one side, beyond its blocks' own colours.
 *
 * In a block that was modified it is the words that differ, or the whole
 * block when none do. A block only one version has is not listed, in a hunk
 * of either kind: it is new or gone as a whole, and its own colour says so
 * (see `NOTE_DIFF_ATTRIBUTE`), as an added or a removed line of a code diff
 * has no changed words in it.
 */
export function hunkTextRanges(hunk: NoteDiffHunk, side: NoteDiffSide): NoteDiffUnitRanges[] {
  const result: NoteDiffUnitRanges[] = [];
  for (const op of hunk.ops) {
    if (op.type !== 'modified') continue;
    const unitIndex = side === 'previous' ? op.previousIndex : op.currentIndex;
    const ranges = side === 'previous' ? op.previousRanges : op.currentRanges;
    if (op.wholeUnit) result.push({ unitIndex, ranges: null });
    else if (ranges.length > 0) result.push({ unitIndex, ranges });
  }
  return result;
}
