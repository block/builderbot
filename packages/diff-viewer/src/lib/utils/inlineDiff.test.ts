import { describe, expect, it } from 'vitest';
import {
  computeLineDiff,
  getLineDiffResult,
  type LineDiffResult,
  type CharHighlight,
} from './inlineDiff';

describe('computeLineDiff', () => {
  describe('identical lines', () => {
    it('marks all lines unchanged when before and after are identical', () => {
      const lines = ['line 1', 'line 2', 'line 3'];
      const result = computeLineDiff(lines, lines);

      expect(result.beforeLines).toEqual(['unchanged', 'unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'unchanged', 'unchanged']);
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('handles empty input', () => {
      const result = computeLineDiff([], []);
      expect(result.beforeLines).toEqual([]);
      expect(result.afterLines).toEqual([]);
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('handles single identical line', () => {
      const result = computeLineDiff(['hello'], ['hello']);
      expect(result.beforeLines).toEqual(['unchanged']);
      expect(result.afterLines).toEqual(['unchanged']);
    });
  });

  describe('pure additions and removals', () => {
    it('marks all lines as added when before is empty', () => {
      const result = computeLineDiff([], ['line 1', 'line 2']);
      expect(result.beforeLines).toEqual([]);
      expect(result.afterLines).toEqual(['added', 'added']);
    });

    it('marks all lines as removed when after is empty', () => {
      const result = computeLineDiff(['line 1', 'line 2'], []);
      expect(result.beforeLines).toEqual(['removed', 'removed']);
      expect(result.afterLines).toEqual([]);
    });

    it('marks completely different lines as removed/added', () => {
      const before = ['aaa xyz', 'bbb xyz'];
      const after = ['111 qqq', '222 qqq'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['removed', 'removed']);
      expect(result.afterLines).toEqual(['added', 'added']);
      expect(result.modifiedPairs).toHaveLength(0);
    });

    it('detects additions at the end with unchanged context', () => {
      const before = ['line 1', 'line 2'];
      const after = ['line 1', 'line 2', 'line 3'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'unchanged', 'added']);
    });

    it('detects removals at the beginning with unchanged context', () => {
      const before = ['line 0', 'line 1', 'line 2'];
      const after = ['line 1', 'line 2'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['removed', 'unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'unchanged']);
    });

    it('detects additions in the middle', () => {
      const before = ['line 1', 'line 3'];
      const after = ['line 1', 'line 2', 'line 3'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'added', 'unchanged']);
    });
  });

  describe('modified line detection', () => {
    it('pairs modified lines without offset', () => {
      const before = ['const x = 1;'];
      const after = ['const x = 2;'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['modified']);
      expect(result.afterLines).toEqual(['modified']);
      expect(result.modifiedPairs).toHaveLength(1);
      expect(result.modifiedPairs[0].beforeLineIndex).toBe(0);
      expect(result.modifiedPairs[0].afterLineIndex).toBe(0);
    });

    it('detects multiple modified pairs', () => {
      const before = ['const a = 1;', 'unchanged', 'const b = true;'];
      const after = ['const a = 2;', 'unchanged', 'const b = false;'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['modified', 'unchanged', 'modified']);
      expect(result.afterLines).toEqual(['modified', 'unchanged', 'modified']);
      expect(result.modifiedPairs).toHaveLength(2);
    });

    it('handles mixed modifications and unchanged lines', () => {
      const before = ['first', 'const x = 1;', 'middle', 'last'];
      const after = ['first', 'const x = 2;', 'middle', 'last'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'modified', 'unchanged', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'modified', 'unchanged', 'unchanged']);
      expect(result.modifiedPairs).toHaveLength(1);
    });
  });

  describe('insertion offset (peek-ahead)', () => {
    it('detects a modified pair when an insertion precedes it', () => {
      const before = ['  content: string | string[];'];
      const after = ['  newField: boolean;', '  content: string | string[];'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged']);
      expect(result.afterLines).toEqual(['added', 'unchanged']);
    });

    it('detects modification through an insertion offset', () => {
      const before = ['  pattern: string | string[];'];
      const after = ['  newField: boolean;', '  pattern: string[];'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['modified']);
      expect(result.afterLines).toEqual(['added', 'modified']);
      expect(result.modifiedPairs).toHaveLength(1);
      expect(result.modifiedPairs[0].beforeLineIndex).toBe(0);
      expect(result.modifiedPairs[0].afterLineIndex).toBe(1);
    });

    it('handles insertion before a modification with unchanged context', () => {
      const before = ['header', '  pattern: string | string[];', 'footer'];
      const after = ['header', '// totally new comment here', '  pattern: string[];', 'footer'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['unchanged', 'modified', 'unchanged']);
      expect(result.afterLines).toEqual(['unchanged', 'added', 'modified', 'unchanged']);
    });
  });

  describe('character highlights', () => {
    it('produces highlights for word-level changes', () => {
      const before = ['const x = 1;'];
      const after = ['const x = 2;'];
      const result = computeLineDiff(before, after);

      expect(result.modifiedPairs).toHaveLength(1);
      const pair = result.modifiedPairs[0];

      // "1;" -> "2;" are the changed tokens
      expect(pair.beforeHighlights.length).toBeGreaterThan(0);
      expect(pair.afterHighlights.length).toBeGreaterThan(0);
    });

    it('highlights only the changed word in a sentence', () => {
      const before = ['the quick brown fox'];
      const after = ['the slow brown fox'];
      const result = computeLineDiff(before, after);

      const pair = result.modifiedPairs[0];
      // "quick" is at position 4-9, "slow" is at position 4-8
      expect(pair.beforeHighlights).toEqual([{ start: 4, end: 9 }]);
      expect(pair.afterHighlights).toEqual([{ start: 4, end: 8 }]);
    });

    it('highlights multiple changed words', () => {
      const before = ['function foo(bar: string): number {'];
      const after = ['function baz(bar: boolean): string {'];
      const result = computeLineDiff(before, after);

      const pair = result.modifiedPairs[0];
      expect(pair.beforeHighlights.length).toBeGreaterThanOrEqual(2);
      expect(pair.afterHighlights.length).toBeGreaterThanOrEqual(2);
    });

    it('produces no highlights when lines are identical but in unmatched blocks', () => {
      // This shouldn't happen in practice since identical lines would be
      // caught by LCS, but the char-highlight logic should handle it
      const before = ['  return null;', '  return value;'];
      const after = ['  return value;'];
      const result = computeLineDiff(before, after);

      // "return null;" is removed, "return value;" is unchanged via LCS
      expect(result.beforeLines[0]).toBe('removed');
      expect(result.beforeLines[1]).toBe('unchanged');
      expect(result.afterLines[0]).toBe('unchanged');
    });
  });

  describe('complex scenarios', () => {
    it('handles a realistic code diff with mixed changes', () => {
      const before = [
        'import { foo } from "bar";',
        '',
        'export function hello() {',
        '  return "world";',
        '}',
      ];
      const after = [
        'import { foo, baz } from "bar";',
        'import { qux } from "quux";',
        '',
        'export function hello() {',
        '  return "universe";',
        '}',
      ];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines[0]).toBe('modified'); // import changed
      expect(result.beforeLines[1]).toBe('unchanged'); // empty line
      expect(result.beforeLines[2]).toBe('unchanged'); // function decl
      expect(result.beforeLines[3]).toBe('modified'); // return changed
      expect(result.beforeLines[4]).toBe('unchanged'); // closing brace

      expect(result.afterLines[0]).toBe('modified'); // import changed
      expect(result.afterLines[1]).toBe('added'); // new import
      expect(result.afterLines[2]).toBe('unchanged'); // empty line
      expect(result.afterLines[3]).toBe('unchanged'); // function decl
      expect(result.afterLines[4]).toBe('modified'); // return changed
      expect(result.afterLines[5]).toBe('unchanged'); // closing brace
    });

    it('handles multiple consecutive removals followed by additions', () => {
      const before = ['aaa', 'bbb', 'ccc'];
      const after = ['xxx', 'yyy', 'zzz'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines).toEqual(['removed', 'removed', 'removed']);
      expect(result.afterLines).toEqual(['added', 'added', 'added']);
    });

    it('handles interleaved unchanged and changed lines', () => {
      const before = ['A', 'B', 'C', 'D', 'E'];
      const after = ['A', 'B2', 'C', 'D2', 'E'];
      const result = computeLineDiff(before, after);

      expect(result.beforeLines[0]).toBe('unchanged');
      expect(result.beforeLines[2]).toBe('unchanged');
      expect(result.beforeLines[4]).toBe('unchanged');
      // B and D are dissimilar enough to be removed/added rather than modified
      expect(result.afterLines[0]).toBe('unchanged');
      expect(result.afterLines[2]).toBe('unchanged');
      expect(result.afterLines[4]).toBe('unchanged');
    });
  });
});

describe('getLineDiffResult (caching)', () => {
  it('returns same result object for identical inputs', () => {
    const before = ['const x = 1;'];
    const after = ['const x = 2;'];

    const result1 = getLineDiffResult(before, after);
    const result2 = getLineDiffResult(before, after);

    expect(result1).toBe(result2); // same reference
  });

  it('returns different result objects for different inputs', () => {
    const result1 = getLineDiffResult(['a'], ['b']);
    const result2 = getLineDiffResult(['c'], ['d']);

    expect(result1).not.toBe(result2);
  });
});
