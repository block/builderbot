import { describe, expect, it } from 'vitest';
import type { RepoHomeItem } from '../../types';
import { normalizeQuery, resultsLabel, searchRepos, visibleSidebarRepos } from './sidebarSearch';

// ── Fixtures ──

function repo(overrides: Partial<RepoHomeItem> = {}): RepoHomeItem {
  return {
    githubRepo: 'org/alpha',
    subpath: '',
    shortName: 'alpha',
    hue: 0,
    createdAt: 0,
    pinned: false,
    pinSortOrder: null,
    defaultBranch: 'main',
    hasLocalClone: true,
    ...overrides,
  };
}

// Pinned-first, like store.list_repos_for_home.
const staged = repo({
  githubRepo: 'block/builderbot',
  subpath: 'apps/staged',
  shortName: 'staged',
  pinned: true,
  pinSortOrder: 0,
});
const g2 = repo({
  githubRepo: 'squareup/g2',
  subpath: 'web',
  shortName: 'g2',
  pinned: true,
  pinSortOrder: 1,
});
const cashIos = repo({
  githubRepo: 'cashapp/cash-ios',
  shortName: 'cash-ios',
  pinned: true,
  pinSortOrder: 2,
});
const differ = repo({
  githubRepo: 'block/builderbot',
  subpath: 'apps/differ',
  shortName: 'differ',
});
const dashboard = repo({ githubRepo: 'squareup/dashboard', shortName: 'dashboard' });
const all = [staged, g2, cashIos, differ, dashboard];

describe('normalizeQuery', () => {
  it('trims and lower-cases, treating whitespace as empty', () => {
    expect(normalizeQuery('  Cash ')).toBe('cash');
    expect(normalizeQuery('   ')).toBe('');
  });
});

describe('searchRepos', () => {
  it('returns the same array for an empty or whitespace query', () => {
    expect(searchRepos(all, '')).toBe(all);
    expect(searchRepos(all, '  ')).toBe(all);
  });

  it('matches the short name case-insensitively', () => {
    expect(searchRepos(all, 'CASH')).toEqual([cashIos]);
    expect(searchRepos(all, 'Board')).toEqual([dashboard]);
  });

  it('matches the github repo path', () => {
    expect(searchRepos(all, 'builderbot')).toEqual([staged, differ]);
    expect(searchRepos(all, 'squareup/')).toEqual([g2, dashboard]);
  });

  it('matches across the repo/subpath boundary', () => {
    expect(searchRepos(all, 'builderbot/apps/sta')).toEqual([staged]);
    expect(searchRepos(all, 'g2/web')).toEqual([g2]);
  });

  it('does not invent a trailing slash for repos without a subpath', () => {
    expect(searchRepos(all, 'cash-ios/')).toEqual([]);
  });
});

describe('visibleSidebarRepos', () => {
  it('shows only pinned repos when not searching, in input order', () => {
    expect(visibleSidebarRepos(all, '')).toEqual([staged, g2, cashIos]);
  });

  it('narrows the pinned repos by the query, keeping pin order', () => {
    expect(visibleSidebarRepos(all, 'builderbot')).toEqual([staged]);
    expect(visibleSidebarRepos(all, 'squareup')).toEqual([g2]);
    expect(visibleSidebarRepos(all, 's')).toEqual([staged, g2, cashIos]);
  });

  it('never surfaces unpinned repos, even when only they match', () => {
    expect(visibleSidebarRepos(all, 'differ')).toEqual([]);
    expect(visibleSidebarRepos(all, 'dashboard')).toEqual([]);
  });

  it('returns no repos when nothing matches', () => {
    expect(visibleSidebarRepos(all, 'zzz')).toEqual([]);
  });
});

describe('resultsLabel', () => {
  it('pluralises everything but one', () => {
    expect(resultsLabel(0)).toBe('0 results');
    expect(resultsLabel(1)).toBe('1 result');
    expect(resultsLabel(2)).toBe('2 results');
  });
});
