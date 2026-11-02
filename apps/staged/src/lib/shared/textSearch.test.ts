import { describe, expect, it } from 'vitest';
import { matchesAllTerms, matchesQuery, queryTerms } from './textSearch';

describe('queryTerms', () => {
  it('splits on any whitespace and lower-cases', () => {
    expect(queryTerms('  One\tTWO  three ')).toEqual(['one', 'two', 'three']);
  });

  it('is empty for a blank query', () => {
    expect(queryTerms('')).toEqual([]);
    expect(queryTerms('   ')).toEqual([]);
  });
});

describe('matchesQuery', () => {
  it('matches every term anywhere in the field, in any order', () => {
    expect(matchesQuery('one three', 'one two three')).toBe(true);
    expect(matchesQuery('three one', 'one two three')).toBe(true);
  });

  it('requires every term to match', () => {
    expect(matchesQuery('one four', 'one two three')).toBe(false);
  });

  it('keeps single-term substring matching', () => {
    expect(matchesQuery('two', 'one two three')).toBe(true);
    expect(matchesQuery('wo th', 'one two three')).toBe(true);
    expect(matchesQuery('four', 'one two three')).toBe(false);
  });

  it('is case-insensitive in both the query and the field', () => {
    expect(matchesQuery('ONE Three', 'One Two THREE')).toBe(true);
  });

  it('matches everything for a blank query', () => {
    expect(matchesQuery('', 'anything')).toBe(true);
    expect(matchesQuery('  ', null)).toBe(true);
  });

  it('lets different terms land in different fields', () => {
    expect(matchesQuery('staged builderbot', 'staged', 'block/builderbot')).toBe(true);
  });

  it('skips null, undefined and empty fields', () => {
    expect(matchesQuery('x', null, undefined, '')).toBe(false);
    expect(matchesQuery('x', null, 'x')).toBe(true);
  });
});

describe('matchesAllTerms', () => {
  it('takes pre-split terms', () => {
    expect(matchesAllTerms(['one', 'three'], 'one two three')).toBe(true);
    expect(matchesAllTerms([], 'one two three')).toBe(true);
    expect(matchesAllTerms(['one', 'four'], 'one two three')).toBe(false);
  });
});
