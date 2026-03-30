import { describe, expect, it } from 'vitest';
import { getLineClass, getCharHighlights } from './diffViewerHelpers';
import type { ChangedAlignmentEntry } from './diffViewerHelpers';
import type { Alignment } from '../types';

/**
 * Helper to build alignment lookup maps and the changedAlignments array
 * from a full alignment array. Maps line indices to the index within
 * the changedAlignments array (matching production code).
 */
function buildLookups(alignments: Alignment[]) {
  const beforeLineToAlignment = new Map<number, number>();
  const afterLineToAlignment = new Map<number, number>();
  const changedAlignments: ChangedAlignmentEntry[] = [];

  for (let i = 0; i < alignments.length; i++) {
    const a = alignments[i];
    if (a.changed) {
      const changedIdx = changedAlignments.length;
      changedAlignments.push({ alignment: a, index: i });
      for (let line = a.before.start; line < a.before.end; line++) {
        beforeLineToAlignment.set(line, changedIdx);
      }
      for (let line = a.after.start; line < a.after.end; line++) {
        afterLineToAlignment.set(line, changedIdx);
      }
    }
  }

  return { beforeLineToAlignment, afterLineToAlignment, changedAlignments };
}

describe('getLineClass', () => {
  it('returns null for lines not in a changed alignment', () => {
    const alignments: Alignment[] = [
      { before: { start: 0, end: 3 }, after: { start: 0, end: 3 }, changed: false },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getLineClass(
      'before', 1, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, ['a', 'b', 'c'], ['a', 'b', 'c'],
    );
    expect(result).toBeNull();
  });

  it('returns "modified" for a modified before-line in a changed alignment', () => {
    const beforeLines = ['const x = 1;', 'unchanged'];
    const afterLines = ['const x = 2;', 'unchanged'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: true },
      { before: { start: 1, end: 2 }, after: { start: 1, end: 2 }, changed: false },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getLineClass(
      'before', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).toBe('modified');
  });

  it('returns "modified" for a modified after-line', () => {
    const beforeLines = ['const x = 1;'];
    const afterLines = ['const x = 2;'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: true },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getLineClass(
      'after', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).toBe('modified');
  });

  it('returns "removed" for a deleted before-line', () => {
    const beforeLines = ['deleted line', 'kept line'];
    const afterLines = ['kept line'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 0 }, changed: true },
      { before: { start: 1, end: 2 }, after: { start: 0, end: 1 }, changed: false },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getLineClass(
      'before', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).toBe('removed');
  });

  it('returns "added" for a new after-line', () => {
    const beforeLines = ['kept line'];
    const afterLines = ['kept line', 'new line'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: false },
      { before: { start: 1, end: 1 }, after: { start: 1, end: 2 }, changed: true },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getLineClass(
      'after', 1, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).toBe('added');
  });

  it('correctly maps line indices within multi-line changed alignments', () => {
    // Alignment covers lines 2-4 in before, 2-5 in after
    const beforeLines = ['ctx', 'ctx', 'const a = 1;', 'const b = true;', 'ctx'];
    const afterLines = ['ctx', 'ctx', 'const a = 2;', 'newLine();', 'const b = false;', 'ctx'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 2 }, after: { start: 0, end: 2 }, changed: false },
      { before: { start: 2, end: 4 }, after: { start: 2, end: 5 }, changed: true },
      { before: { start: 4, end: 5 }, after: { start: 5, end: 6 }, changed: false },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    // Line 2 before: "const a = 1;" should be modified (similar to "const a = 2;")
    expect(getLineClass(
      'before', 2, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    )).toBe('modified');

    // Line 3 in after: "newLine();" should be added (no similar before-line)
    expect(getLineClass(
      'after', 3, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    )).toBe('added');
  });
});

describe('getCharHighlights', () => {
  it('returns null for lines not in a changed alignment', () => {
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: false },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getCharHighlights(
      'before', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, ['hello'], ['hello'],
    );
    expect(result).toBeNull();
  });

  it('returns null for non-modified lines in a changed alignment', () => {
    const beforeLines = ['removed line'];
    const afterLines = ['totally different content here'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: true },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    // Lines are too dissimilar to be "modified", so no char highlights
    const result = getCharHighlights(
      'before', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).toBeNull();
  });

  it('returns highlights for a modified before-line', () => {
    const beforeLines = ['const x = 1;'];
    const afterLines = ['const x = 2;'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: true },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getCharHighlights(
      'before', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).not.toBeNull();
    expect(result!.length).toBeGreaterThan(0);
    // The highlight should cover "1;" (the changed part)
    for (const h of result!) {
      expect(h.start).toBeGreaterThanOrEqual(0);
      expect(h.end).toBeGreaterThan(h.start);
      expect(h.end).toBeLessThanOrEqual(beforeLines[0].length);
    }
  });

  it('returns highlights for a modified after-line', () => {
    const beforeLines = ['the quick brown fox'];
    const afterLines = ['the slow brown fox'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: true },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getCharHighlights(
      'after', 0, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).not.toBeNull();
    // "slow" replaces "quick" -> highlight at position 4-8
    expect(result).toEqual([{ start: 4, end: 8 }]);
  });

  it('works with offset line indices in alignments', () => {
    const beforeLines = ['ctx', 'const x = 1;'];
    const afterLines = ['ctx', 'const x = 2;'];
    const alignments: Alignment[] = [
      { before: { start: 0, end: 1 }, after: { start: 0, end: 1 }, changed: false },
      { before: { start: 1, end: 2 }, after: { start: 1, end: 2 }, changed: true },
    ];
    const { beforeLineToAlignment, afterLineToAlignment, changedAlignments } = buildLookups(alignments);

    const result = getCharHighlights(
      'after', 1, beforeLineToAlignment, afterLineToAlignment,
      changedAlignments, beforeLines, afterLines,
    );
    expect(result).not.toBeNull();
    expect(result!.length).toBeGreaterThan(0);
  });
});
