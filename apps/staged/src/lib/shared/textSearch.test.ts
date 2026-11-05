import { describe, expect, it } from 'vitest';
import { highlightSegments, matchesAllTerms, queryTerms, termPrNumber } from './textSearch';

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

describe('termPrNumber', () => {
  it('reads bare, #- and pr-prefixed digits', () => {
    expect(termPrNumber('123')).toBe(123);
    expect(termPrNumber('#123')).toBe(123);
    expect(termPrNumber('pr123')).toBe(123);
    expect(termPrNumber('pr#123')).toBe(123);
    expect(termPrNumber('#007')).toBe(7);
  });

  it('rejects anything else', () => {
    expect(termPrNumber('#')).toBeNull();
    expect(termPrNumber('pr')).toBeNull();
    expect(termPrNumber('pr#')).toBeNull();
    expect(termPrNumber('12a')).toBeNull();
    expect(termPrNumber('#pr12')).toBeNull();
    expect(termPrNumber('xpr12')).toBeNull();
    expect(termPrNumber('##12')).toBeNull();
    expect(termPrNumber('')).toBeNull();
  });
});

describe('highlightSegments', () => {
  it('marks every occurrence of every term, case-insensitively', () => {
    expect(highlightSegments('Fix login, then LOGIN again', ['login'])).toEqual([
      { text: 'Fix ', match: false },
      { text: 'login', match: true },
      { text: ', then ', match: false },
      { text: 'LOGIN', match: true },
      { text: ' again', match: false },
    ]);
  });

  it('merges overlapping and adjacent matches into one run', () => {
    expect(highlightSegments('fix-login', ['fix-l', 'login'])).toEqual([
      { text: 'fix-login', match: true },
    ]);
    expect(highlightSegments('abcd', ['ab', 'cd'])).toEqual([{ text: 'abcd', match: true }]);
  });

  it('returns the whole text unmatched for no terms or no hits', () => {
    expect(highlightSegments('fix-login', [])).toEqual([{ text: 'fix-login', match: false }]);
    expect(highlightSegments('fix-login', ['zzz'])).toEqual([{ text: 'fix-login', match: false }]);
  });

  it('is empty for empty text', () => {
    expect(highlightSegments('', ['x'])).toEqual([]);
  });

  it('marks a #-prefixed number term including its #', () => {
    expect(highlightSegments('#123 · fix', ['#123'])).toEqual([
      { text: '#123', match: true },
      { text: ' · fix', match: false },
    ]);
  });

  it('still lands highlights when lower-casing changes the length', () => {
    // 'İ' lower-cases to 'i̇' (i + combining dot), two code units, so every
    // offset after it in the lower-cased text is one past the original.
    expect('İ'.toLowerCase()).toHaveLength(2);
    expect(highlightSegments('İstanbul fix', ['fix'])).toEqual([
      { text: 'İstanbul ', match: false },
      { text: 'fix', match: true },
    ]);
    expect(highlightSegments('Fix İzmir, then fix İzmit', ['fix', 'zmi'])).toEqual([
      { text: 'Fix', match: true },
      { text: ' İ', match: false },
      { text: 'zmi', match: true },
      { text: 'r, then ', match: false },
      { text: 'fix', match: true },
      { text: ' İ', match: false },
      { text: 'zmi', match: true },
      { text: 't', match: false },
    ]);
  });

  it('highlights a whole code point when a match covers part of its lower case', () => {
    // A query typed with the 'İ' lower-cases the same way.
    expect(highlightSegments('İstanbul', queryTerms('İst'))).toEqual([
      { text: 'İst', match: true },
      { text: 'anbul', match: false },
    ]);
    // 'i' matches only the first half of the lower-cased 'İ'.
    expect(highlightSegments('aİb', ['i'])).toEqual([
      { text: 'a', match: false },
      { text: 'İ', match: true },
      { text: 'b', match: false },
    ]);
  });

  it('keeps offsets right around astral characters and a final sigma', () => {
    expect(highlightSegments('😀 İ fix', ['fix'])).toEqual([
      { text: '😀 İ ', match: false },
      { text: 'fix', match: true },
    ]);
    // Whole-string lower-casing makes a word-final Σ 'ς', as queryTerms does
    // for the query, so a final-sigma term still matches.
    expect(highlightSegments('İ ΟΔΟΣ', ['οδος'])).toEqual([
      { text: 'İ ', match: false },
      { text: 'ΟΔΟΣ', match: true },
    ]);
  });
});
