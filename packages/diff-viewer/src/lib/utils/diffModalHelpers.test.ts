import { describe, expect, it } from 'vitest';
import { pathsMatch } from './diffModalHelpers';

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
