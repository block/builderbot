import { describe, expect, it } from 'vitest';
import { computeLineDiff } from './inlineDiff';

describe('computeLineDiff', () => {
  it('detects a modified pair when an insertion precedes it', () => {
    const before = ['  content: string | string[];'];
    const after = ['  newField: boolean;', '  content: string | string[];'];

    const result = computeLineDiff(before, after);

    // The identical line should be matched by LCS, leaving nothing unmatched
    expect(result.beforeLines).toEqual(['unchanged']);
    expect(result.afterLines).toEqual(['added', 'unchanged']);
  });

  it('detects modification through an insertion offset', () => {
    const before = ['  pattern: string | string[];'];
    const after = ['  newField: boolean;', '  pattern: string[];'];

    const result = computeLineDiff(before, after);

    // The before line should pair with the similar after line, not be marked removed
    expect(result.beforeLines).toEqual(['modified']);
    expect(result.afterLines).toEqual(['added', 'modified']);
    expect(result.modifiedPairs).toHaveLength(1);
    expect(result.modifiedPairs[0].beforeLineIndex).toBe(0);
    expect(result.modifiedPairs[0].afterLineIndex).toBe(1);
  });

  it('still marks pure removals and additions correctly', () => {
    const before = ['line A', 'line B'];
    const after = ['line C', 'line D'];

    const result = computeLineDiff(before, after);

    expect(result.beforeLines).toEqual(['removed', 'removed']);
    expect(result.afterLines).toEqual(['added', 'added']);
    expect(result.modifiedPairs).toHaveLength(0);
  });

  it('pairs modified lines without offset correctly', () => {
    const before = ['const x = 1;'];
    const after = ['const x = 2;'];

    const result = computeLineDiff(before, after);

    expect(result.beforeLines).toEqual(['modified']);
    expect(result.afterLines).toEqual(['modified']);
    expect(result.modifiedPairs).toHaveLength(1);
    expect(result.modifiedPairs[0].beforeHighlights.length).toBeGreaterThan(0);
    expect(result.modifiedPairs[0].afterHighlights.length).toBeGreaterThan(0);
  });
});
