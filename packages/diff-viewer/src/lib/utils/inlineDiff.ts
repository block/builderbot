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

function similarity(a: string, b: string): number {
  if (a.length === 0 && b.length === 0) return 1;
  if (a.length === 0 || b.length === 0) return 0;
  const product = a.length * b.length;
  if (product > 1_000_000) {
    console.info('[diff] similarity: large line pair', { aLen: a.length, bLen: b.length, product });
  }
  const t0 = product > 1_000_000 ? performance.now() : 0;
  const lcsLen = lcsLength(a, b);
  if (t0 > 0) {
    console.info('[diff] similarity done', { elapsed: `${(performance.now() - t0).toFixed(1)}ms` });
  }
  return (2 * lcsLen) / (a.length + b.length);
}

const SIMILARITY_THRESHOLD = 0.55;

function splitWords(text: string): string[] {
  return text.match(/[\p{L}\p{N}_]+|\s+|[^\s\p{L}\p{N}_]/gu) ?? [];
}

function computeCharHighlights(
  before: string,
  after: string,
): { beforeHighlights: CharHighlight[]; afterHighlights: CharHighlight[] } {
  const t0 = performance.now();
  const beforeTokens = splitWords(before);
  const afterTokens = splitWords(after);
  if (beforeTokens.length * afterTokens.length > 1_000_000) {
    console.info('[diff] computeCharHighlights: large token pair', { beforeTokens: beforeTokens.length, afterTokens: afterTokens.length, beforeLen: before.length, afterLen: after.length });
  }

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
): LineDiffResult {
  const t0 = performance.now();
  const beforeMaxLen = Math.max(0, ...beforeLines.map(l => l.length));
  const afterMaxLen = Math.max(0, ...afterLines.map(l => l.length));
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

  while (bi < unmatchedBefore.length && ai < unmatchedAfter.length) {
    const bIdx = unmatchedBefore[bi];
    const aIdx = unmatchedAfter[ai];
    const sim = similarity(beforeLines[bIdx].trim(), afterLines[aIdx].trim());

    if (sim > SIMILARITY_THRESHOLD) {
      beforeClasses[bIdx] = 'modified';
      afterClasses[aIdx] = 'modified';
      const { beforeHighlights, afterHighlights } = computeCharHighlights(
        beforeLines[bIdx],
        afterLines[aIdx],
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
        const scanSim = similarity(beforeLines[bIdx].trim(), afterLines[scanIdx].trim());
        if (scanSim > SIMILARITY_THRESHOLD) {
          // Mark all skipped after-lines as pure additions.
          for (let skip = ai; skip < scan; skip++) {
            afterClasses[unmatchedAfter[skip]] = 'added';
          }
          ai = scan;
          found = true;
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
  console.info('[diff] computeLineDiff done', { modifiedPairs: modifiedPairs.length, elapsed: `${elapsed.toFixed(1)}ms` });

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

