import { describe, expect, it } from 'vitest';
import { buildFileEntries, fileChangeScale, fileChangeTotal, pathsMatch } from './diffModalHelpers';

describe('pathsMatch', () => {
  it('returns true for identical paths', () => {
    expect(pathsMatch('src/foo.ts', 'src/foo.ts')).toBe(true);
  });

  it('returns true when first path is a suffix of the second at a / boundary', () => {
    expect(pathsMatch('prefix/src/foo.ts', 'src/foo.ts')).toBe(true);
  });

  it('returns true when second path is a suffix of the first at a / boundary', () => {
    expect(pathsMatch('src/foo.ts', 'prefix/src/foo.ts')).toBe(true);
  });

  it('returns false for completely different paths', () => {
    expect(pathsMatch('src/foo.ts', 'src/bar.ts')).toBe(false);
  });

  it('returns false when suffix matches but not at a / boundary', () => {
    expect(pathsMatch('foo/bar.ts', 'baz/foobar.ts')).toBe(false);
  });

  it('returns false when suffix matches but not at a / boundary (reversed)', () => {
    expect(pathsMatch('baz/foobar.ts', 'foo/bar.ts')).toBe(false);
  });

  it('handles deeply nested path prefixes', () => {
    expect(pathsMatch('a/b/c/d/file.ts', 'c/d/file.ts')).toBe(true);
    expect(pathsMatch('c/d/file.ts', 'a/b/c/d/file.ts')).toBe(true);
  });

  it('returns true for single filename suffix match', () => {
    expect(pathsMatch('src/file.ts', 'file.ts')).toBe(true);
    expect(pathsMatch('file.ts', 'src/file.ts')).toBe(true);
  });

  it('returns false when one path is empty', () => {
    expect(pathsMatch('', 'foo.ts')).toBe(false);
    expect(pathsMatch('foo.ts', '')).toBe(false);
  });

  it('returns true when both paths are empty', () => {
    expect(pathsMatch('', '')).toBe(true);
  });
});

describe('fileChangeTotal', () => {
  it('returns null when a summary has no line stats', () => {
    expect(fileChangeTotal({ before: 'a.ts', after: 'a.ts' })).toBeNull();
  });

  it('adds available added and deleted counts', () => {
    expect(fileChangeTotal({ before: 'a.ts', after: 'a.ts', addedLines: 5, deletedLines: 2 })).toBe(
      7
    );
  });
});

describe('fileChangeScale', () => {
  it('returns 0 when totals are missing or empty', () => {
    expect(fileChangeScale(null, 10)).toBe(0);
    expect(fileChangeScale(0, 10)).toBe(0);
    expect(fileChangeScale(10, 0)).toBe(0);
  });

  it('uses logarithmic scaling against the largest file total', () => {
    expect(fileChangeScale(9, 99)).toBeCloseTo(Math.log1p(9) / Math.log1p(99));
    expect(fileChangeScale(99, 99)).toBe(1);
  });
});

describe('buildFileEntries', () => {
  it('propagates optional line stats to file entries', () => {
    const entries = buildFileEntries(
      [{ before: 'src/file.ts', after: 'src/file.ts', addedLines: 3, deletedLines: 1 }],
      [],
      []
    );

    expect(entries[0]).toMatchObject({
      path: 'src/file.ts',
      addedLines: 3,
      deletedLines: 1,
    });
  });

  it('keeps missing line stats as null for older cached summaries', () => {
    const entries = buildFileEntries([{ before: 'src/file.ts', after: 'src/file.ts' }], [], []);

    expect(entries[0]).toMatchObject({
      addedLines: null,
      deletedLines: null,
    });
  });
});
