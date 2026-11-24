export type BeforeLineClass = 'removed' | 'modified' | 'unchanged';
export type AfterLineClass = 'added' | 'modified' | 'unchanged';

export interface CharHighlight {
  start: number;
  end: number;
}

export interface ModifiedPair {
  beforeLineIndex: number;
  afterLineIndex: number;
  beforeHighlights: CharHighlight[];
  afterHighlights: CharHighlight[];
}

export interface LineDiffResult {
  beforeLines: BeforeLineClass[];
  afterLines: AfterLineClass[];
  modifiedPairs: ModifiedPair[];
}

/**
 * No one comparison takes more than this share of what is left of a budget:
 * an eighth. The comparisons that cost most are of long lines with little in
 * common, and they find nothing. Refusing those first leaves the tail of the
 * budget to the cheap ones, which are the lines that changed by a few words.
 */
const WORK_BUDGET_SHARE = 8;

/**
 * A cap on the comparisons a diff makes between lines that are not the same.
 *
 * Matching equal lines is cheap. Pairing the rest is not: every unmatched
 * line is compared by character with up to eleven others, and each pair
 * found is then compared by word. Both fill a table with a cell for every
 * pair of characters, or of words, so the cost grows with the number of
 * changed lines times the square of their length.
 *
 * The budget is counted in those cells: about four nanoseconds each. It is a
 * property of the input and not of the machine or the clock, so the same
 * input always diffs the same way. A comparison that does not fit is not
 * made, and its lines are taken to have nothing in common: they stay a
 * removal and an addition, where they might have been a modified pair.
 *
 * One budget can cover several calls, and a caller's own comparisons through
 * `spend`.
 */
export class DiffWorkBudget {
  /** The cells filled so far. */
  spent = 0;
  /** A comparison was refused: the diff is not what it would be without a cap. */
  capped = false;

  constructor(readonly limit: number) {}

  /** Pay for a comparison of `cells`, or learn that it is not to be made. */
  spend(cells: number): boolean {
    if (cells * WORK_BUDGET_SHARE > this.limit - this.spent) {
      this.capped = true;
      return false;
    }
    this.spent += cells;
    return true;
  }
}

export interface LineDiffOptions {
  /** A cap on pairing the unmatched lines. Without one there is no cap. */
  budget?: DiffWorkBudget;
}

function lcsIndices<T>(
  a: T[],
  b: T[],
  eq: (x: T, y: T) => boolean = (x, y) => x === y,
): { aIndices: Set<number>; bIndices: Set<number> } {
  const m = a.length;
  const n = b.length;
  const dp: number[][] = Array.from({ length: m + 1 }, () =>
    new Array<number>(n + 1).fill(0),
  );

  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      if (eq(a[i - 1], b[j - 1])) {
        dp[i][j] = dp[i - 1][j - 1] + 1;
      } else {
        dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
      }
    }
  }

  const aIndices = new Set<number>();
  const bIndices = new Set<number>();
  let i = m;
  let j = n;
  while (i > 0 && j > 0) {
    if (eq(a[i - 1], b[j - 1])) {
      aIndices.add(i - 1);
      bIndices.add(j - 1);
      i--;
      j--;
    } else if (dp[i - 1][j] >= dp[i][j - 1]) {
      i--;
    } else {
      j--;
    }
  }

  return { aIndices, bIndices };
}

function lcsLength(a: string, b: string): number {
  const m = a.length;
  const n = b.length;
  const prev = new Array<number>(n + 1).fill(0);
  const curr = new Array<number>(n + 1).fill(0);

  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      if (a[i - 1] === b[j - 1]) {
        curr[j] = prev[j - 1] + 1;
      } else {
        curr[j] = Math.max(prev[j], curr[j - 1]);
      }
    }
    for (let j = 0; j <= n; j++) {
      prev[j] = curr[j];
      curr[j] = 0;
    }
  }

  return prev[n];
}

/**
 * The largest pair the quadratic passes below take on: two lines whose
 * lengths multiply past this are not compared for similarity, and two token
 * lists past it are highlighted whole.
 */
export const LCS_BAILOUT_PRODUCT = 1_000_000;

/**
 * How alike two lines are, from 0 to 1. Two that the budget has no room to
 * compare are not alike.
 *
 * What the lines start and end with in common is part of their longest
 * common subsequence whatever lies between, so only the middles are
 * compared, and only they are paid for: a long line with one word changed
 * costs next to nothing, and two unrelated lines cost their whole product.
 */
function similarity(a: string, b: string, budget: DiffWorkBudget): number {
  if (a.length === 0 && b.length === 0) return 1;
  if (a.length === 0 || b.length === 0) return 0;
  const product = a.length * b.length;
  if (product > LCS_BAILOUT_PRODUCT) {
    console.info('[diff] similarity: bailing out on large line pair', { aLen: a.length, bLen: b.length, product });
    return 0;
  }
  const shorter = Math.min(a.length, b.length);
  let prefix = 0;
  while (prefix < shorter && a[prefix] === b[prefix]) prefix++;
  let suffix = 0;
  while (
    suffix < shorter - prefix &&
    a[a.length - 1 - suffix] === b[b.length - 1 - suffix]
  ) {
    suffix++;
  }
  const common = prefix + suffix;
  if (!budget.spend((a.length - common) * (b.length - common))) return 0;
  const lcsLen =
    common +
    lcsLength(a.slice(prefix, a.length - suffix), b.slice(prefix, b.length - suffix));
  return (2 * lcsLen) / (a.length + b.length);
}

const SIMILARITY_THRESHOLD = 0.55;

function splitWords(text: string): string[] {
  return text.match(/[\p{L}\p{N}_]+|\s+|[^\s\p{L}\p{N}_]/gu) ?? [];
}

/**
 * The word-level difference between two strings: the ranges of each that the
 * other does not hold. `computeLineDiff` runs this on every pair it calls
 * modified; it is exported for callers that pair lines themselves.
 */
export function computeCharHighlights(
  before: string,
  after: string,
): { beforeHighlights: CharHighlight[]; afterHighlights: CharHighlight[] } {
  return highlightTokens(before, after, splitWords(before), splitWords(after));
}

/**
 * What `computeCharHighlights` costs for two strings, in the cells a
 * `DiffWorkBudget` counts: one for every pair of their words.
 */
export function charHighlightsWork(before: string, after: string): number {
  return wordPairs(splitWords(before), splitWords(after));
}

/** Two lists too long to compare are highlighted whole, which fills nothing. */
function wordPairs(beforeTokens: string[], afterTokens: string[]): number {
  const pairs = beforeTokens.length * afterTokens.length;
  return pairs > LCS_BAILOUT_PRODUCT ? 0 : pairs;
}

function highlightTokens(
  before: string,
  after: string,
  beforeTokens: string[],
  afterTokens: string[],
): { beforeHighlights: CharHighlight[]; afterHighlights: CharHighlight[] } {
  if (beforeTokens.length * afterTokens.length > LCS_BAILOUT_PRODUCT) {
    console.info('[diff] computeCharHighlights: bailing out on large token pair', { beforeTokens: beforeTokens.length, afterTokens: afterTokens.length, beforeLen: before.length, afterLen: after.length });
    return {
      beforeHighlights: before.length > 0 ? [{ start: 0, end: before.length }] : [],
      afterHighlights: after.length > 0 ? [{ start: 0, end: after.length }] : [],
    };
  }

  const t0 = performance.now();
  const { aIndices, bIndices } = lcsIndices(beforeTokens, afterTokens);

  const beforeHighlights: CharHighlight[] = [];
  let pos = 0;
  for (let i = 0; i < beforeTokens.length; i++) {
    const tokenLen = beforeTokens[i].length;
    if (!aIndices.has(i)) {
      beforeHighlights.push({ start: pos, end: pos + tokenLen });
    }
    pos += tokenLen;
  }

  const afterHighlights: CharHighlight[] = [];
  pos = 0;
  for (let i = 0; i < afterTokens.length; i++) {
    const tokenLen = afterTokens[i].length;
    if (!bIndices.has(i)) {
      afterHighlights.push({ start: pos, end: pos + tokenLen });
    }
    pos += tokenLen;
  }

  const elapsed = performance.now() - t0;
  if (elapsed > 10) {
    console.info('[diff] computeCharHighlights done', { elapsed: `${elapsed.toFixed(1)}ms`, beforeLen: before.length, afterLen: after.length });
  }

  return {
    beforeHighlights: mergeHighlights(beforeHighlights),
    afterHighlights: mergeHighlights(afterHighlights),
  };
}

function mergeHighlights(highlights: CharHighlight[]): CharHighlight[] {
  if (highlights.length === 0) return highlights;
  const merged: CharHighlight[] = [highlights[0]];
  for (let i = 1; i < highlights.length; i++) {
    const prev = merged[merged.length - 1];
    const curr = highlights[i];
    if (curr.start <= prev.end) {
      prev.end = Math.max(prev.end, curr.end);
    } else {
      merged.push(curr);
    }
  }
  return merged;
}

const MAX_SCAN_AHEAD = 10;

export function computeLineDiff(
  beforeLines: string[],
  afterLines: string[],
  options: LineDiffOptions = {},
): LineDiffResult {
  const t0 = performance.now();
  const budget = options.budget ?? new DiffWorkBudget(Infinity);
  const spentBefore = budget.spent;
  const beforeMaxLen = beforeLines.reduce((max, l) => Math.max(max, l.length), 0);
  const afterMaxLen = afterLines.reduce((max, l) => Math.max(max, l.length), 0);
  console.info('[diff] computeLineDiff start', { beforeLines: beforeLines.length, afterLines: afterLines.length, beforeMaxLineLen: beforeMaxLen, afterMaxLineLen: afterMaxLen });

  const { aIndices: lcsBeforeIndices, bIndices: lcsAfterIndices } = lcsIndices(
    beforeLines,
    afterLines,
  );

  const beforeClasses: BeforeLineClass[] = new Array(beforeLines.length).fill(
    'unchanged',
  );
  const afterClasses: AfterLineClass[] = new Array(afterLines.length).fill(
    'unchanged',
  );

  const unmatchedBefore: number[] = [];
  const unmatchedAfter: number[] = [];

  for (let i = 0; i < beforeLines.length; i++) {
    if (!lcsBeforeIndices.has(i)) {
      unmatchedBefore.push(i);
    }
  }
  for (let i = 0; i < afterLines.length; i++) {
    if (!lcsAfterIndices.has(i)) {
      unmatchedAfter.push(i);
    }
  }

  const modifiedPairs: ModifiedPair[] = [];
  let bi = 0;
  let ai = 0;
  // The scan ahead found this pair alike, and it is not compared again.
  let scanned = false;

  while (bi < unmatchedBefore.length && ai < unmatchedAfter.length) {
    const bIdx = unmatchedBefore[bi];
    const aIdx = unmatchedAfter[ai];
    const alike =
      scanned ||
      similarity(beforeLines[bIdx].trim(), afterLines[aIdx].trim(), budget) > SIMILARITY_THRESHOLD;
    scanned = false;

    if (alike) {
      const beforeTokens = splitWords(beforeLines[bIdx]);
      const afterTokens = splitWords(afterLines[aIdx]);
      if (!budget.spend(wordPairs(beforeTokens, afterTokens))) {
        // A pair with no room left for its words is not made.
        beforeClasses[bIdx] = 'removed';
        bi++;
        continue;
      }
      beforeClasses[bIdx] = 'modified';
      afterClasses[aIdx] = 'modified';
      const { beforeHighlights, afterHighlights } = highlightTokens(
        beforeLines[bIdx],
        afterLines[aIdx],
        beforeTokens,
        afterTokens,
      );
      modifiedPairs.push({
        beforeLineIndex: bIdx,
        afterLineIndex: aIdx,
        beforeHighlights,
        afterHighlights,
      });
      bi++;
      ai++;
    } else {
      // Scan ahead in unmatchedAfter to find a match for the current
      // before-line. This handles cases where many insertions precede a
      // modification (e.g. code re-indented after being wrapped in a block).
      let found = false;
      const scanLimit = Math.min(ai + 1 + MAX_SCAN_AHEAD, unmatchedAfter.length);
      for (let scan = ai + 1; scan < scanLimit; scan++) {
        const scanIdx = unmatchedAfter[scan];
        const scanSim = similarity(beforeLines[bIdx].trim(), afterLines[scanIdx].trim(), budget);
        if (scanSim > SIMILARITY_THRESHOLD) {
          // Mark all skipped after-lines as pure additions.
          for (let skip = ai; skip < scan; skip++) {
            afterClasses[unmatchedAfter[skip]] = 'added';
          }
          ai = scan;
          found = true;
          scanned = true;
          break;
        }
      }
      if (!found) {
        beforeClasses[bIdx] = 'removed';
        bi++;
      }
    }
  }

  while (bi < unmatchedBefore.length) {
    beforeClasses[unmatchedBefore[bi]] = 'removed';
    bi++;
  }
  while (ai < unmatchedAfter.length) {
    afterClasses[unmatchedAfter[ai]] = 'added';
    ai++;
  }

  const elapsed = performance.now() - t0;
  console.info('[diff] computeLineDiff done', { modifiedPairs: modifiedPairs.length, work: budget.spent - spentBefore, capped: budget.capped, elapsed: `${elapsed.toFixed(1)}ms` });

  return {
    beforeLines: beforeClasses,
    afterLines: afterClasses,
    modifiedPairs,
  };
}

const MAX_CACHE_SIZE = 100;

function makeCacheKey(beforeLines: string[], afterLines: string[]): string {
  return beforeLines.join('\n') + '\0' + afterLines.join('\n');
}

export interface LineDiffCache {
  get(beforeLines: string[], afterLines: string[]): LineDiffResult;
}

export function createLineDiffCache(): LineDiffCache {
  const cache = new Map<string, LineDiffResult>();

  return {
    get(beforeLines: string[], afterLines: string[]): LineDiffResult {
      const key = makeCacheKey(beforeLines, afterLines);
      const cached = cache.get(key);
      if (cached) return cached;

      const result = computeLineDiff(beforeLines, afterLines);

      if (cache.size >= MAX_CACHE_SIZE) {
        const firstKey = cache.keys().next().value;
        if (firstKey !== undefined) {
          cache.delete(firstKey);
        }
      }
      cache.set(key, result);

      return result;
    },
  };
}

