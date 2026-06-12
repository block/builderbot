import { describe, expect, it } from 'vitest';
import { matchesRepoSearch } from './repoSearch';

describe('matchesRepoSearch', () => {
  it('matches subpath-only queries', () => {
    expect(matchesRepoSearch('squareup/cash-server', 'kgoose', 'kgoose')).toBe(true);
  });

  it('matches mixed repo and subpath queries', () => {
    expect(matchesRepoSearch('squareup/cash-server', 'kgoose', 'cash kgoose')).toBe(true);
  });

  it('matches joined repo and subpath queries', () => {
    expect(matchesRepoSearch('squareup/cash-server', 'kgoose', 'cash-server/kgoose')).toBe(true);
  });

  it('matches tokens regardless of order', () => {
    expect(matchesRepoSearch('squareup/cash-server', 'kgoose', 'kgoose cash')).toBe(true);
  });

  it('handles missing subpaths', () => {
    expect(matchesRepoSearch('squareup/cash-server', null, 'cash-server')).toBe(true);
    expect(matchesRepoSearch('squareup/cash-server', null, 'kgoose')).toBe(false);
  });

  it('requires every token to match some repo term', () => {
    expect(matchesRepoSearch('squareup/cash-server', 'kgoose', 'cash unknown')).toBe(false);
  });
});
