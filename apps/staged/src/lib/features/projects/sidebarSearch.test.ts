import { describe, expect, it } from 'vitest';
import type { Project, RepoHomeItem } from '../../types';
import { repoKey } from './repoReorder';
import {
  matchingProjectIds,
  matchingRepoKeys,
  normalizeQuery,
  resultsLabel,
  searchRepos,
  sidebarPinnedRepos,
} from './sidebarSearch';

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

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p1',
    name: 'Alpha',
    githubRepo: 'org/alpha',
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    statusOverride: null,
    ...overrides,
  };
}

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

describe('sidebarPinnedRepos', () => {
  it('keeps only pinned repos, in input (pin) order', () => {
    expect(sidebarPinnedRepos(all)).toEqual([staged, g2, cashIos]);
  });

  it('is empty when nothing is pinned', () => {
    expect(sidebarPinnedRepos([differ, dashboard])).toEqual([]);
  });
});

describe('matchingRepoKeys', () => {
  const pinned = sidebarPinnedRepos(all);
  const keys = (...repos: RepoHomeItem[]) => new Set(repos.map(repoKey));

  it('is null for an empty or whitespace query, so every card shows', () => {
    expect(matchingRepoKeys(pinned, '')).toBeNull();
    expect(matchingRepoKeys(pinned, '   ')).toBeNull();
  });

  it('keys the repos searchRepos keeps', () => {
    expect(matchingRepoKeys(pinned, 'builderbot')).toEqual(keys(staged));
    expect(matchingRepoKeys(pinned, 'squareup')).toEqual(keys(g2));
    expect(matchingRepoKeys(pinned, 's')).toEqual(keys(staged, g2, cashIos));
  });

  it('only ever matches the pinned repos it is given', () => {
    expect(matchingRepoKeys(pinned, 'differ')).toEqual(new Set());
    expect(matchingRepoKeys(pinned, 'dashboard')).toEqual(new Set());
  });

  it('is an empty set, not null, when a query matches nothing', () => {
    expect(matchingRepoKeys(pinned, 'zzz')).toEqual(new Set());
  });

  it('tells apart two subpaths of one repo', () => {
    expect(matchingRepoKeys([staged, differ], 'apps/differ')).toEqual(keys(differ));
  });
});

describe('matchingProjectIds', () => {
  const alpha = project({ id: 'p1', name: 'Alpha Service' });
  const beta = project({ id: 'p2', name: 'beta-tools' });
  const projects = [alpha, beta];

  it('is null for an empty or whitespace query, so every row shows', () => {
    expect(matchingProjectIds(projects, '')).toBeNull();
    expect(matchingProjectIds(projects, '  ')).toBeNull();
  });

  it('holds the ids searchProjects keeps', () => {
    expect(matchingProjectIds(projects, 'ALPHA')).toEqual(new Set(['p1']));
    expect(matchingProjectIds(projects, 'a')).toEqual(new Set(['p1', 'p2']));
  });

  it('is an empty set, not null, when a query matches nothing', () => {
    expect(matchingProjectIds(projects, 'zzz')).toEqual(new Set());
  });
});

describe('resultsLabel', () => {
  it('pluralises everything but one', () => {
    expect(resultsLabel(0)).toBe('0 results');
    expect(resultsLabel(1)).toBe('1 result');
    expect(resultsLabel(2)).toBe('2 results');
  });
});
