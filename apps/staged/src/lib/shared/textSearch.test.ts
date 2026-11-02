import { describe, expect, it } from 'vitest';
import { matchesAllTerms, queryTerms } from './textSearch';

describe('queryTerms', () => {
  it('splits on any whitespace and lower-cases', () => {
    expect(queryTerms('  One\tTWO  three ')).toEqual(['one', 'two', 'three']);
  });

  it('is empty for a blank query', () => {
    expect(queryTerms('')).toEqual([]);
    expect(queryTerms('   ')).toEqual([]);
  });
});

describe('matchesAllTerms', () => {
  it('matches every term anywhere in the field, in any order', () => {
    expect(matchesAllTerms(queryTerms('one three'), 'one two three')).toBe(true);
    expect(matchesAllTerms(queryTerms('three one'), 'one two three')).toBe(true);
  });

  it('requires every term to match', () => {
    expect(matchesAllTerms(queryTerms('one four'), 'one two three')).toBe(false);
  });

  it('keeps single-term substring matching', () => {
    expect(matchesAllTerms(queryTerms('two'), 'one two three')).toBe(true);
    expect(matchesAllTerms(queryTerms('wo th'), 'one two three')).toBe(true);
    expect(matchesAllTerms(queryTerms('four'), 'one two three')).toBe(false);
  });

  it('is case-insensitive in both the query and the field', () => {
    expect(matchesAllTerms(queryTerms('ONE Three'), 'One Two THREE')).toBe(true);
  });

  it('matches everything for no terms', () => {
    expect(matchesAllTerms([], 'anything')).toBe(true);
    expect(matchesAllTerms(queryTerms('  '), null)).toBe(true);
  });

  it('lets different terms land in different fields', () => {
    expect(matchesAllTerms(['staged', 'builderbot'], 'staged', 'block/builderbot')).toBe(true);
  });

  it('skips null, undefined and empty fields', () => {
    expect(matchesAllTerms(['x'], null, undefined, '')).toBe(false);
    expect(matchesAllTerms(['x'], null, 'x')).toBe(true);
  });
});
