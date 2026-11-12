import { describe, expect, it } from 'vitest';
import type { Branch, Project, RepoHomeItem } from '../../types';
import { buildSearchSnapshot, type SearchSnapshot } from './projectSearch';
import { repoKey } from './repoReorder';
import {
  matchingRepoKeys,
  resultsLabel,
  searchRepos,
  sidebarPinnedRepos,
  sidebarProjectMatches,
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
    ...overrides,
  };
}

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

  it('matches every term across the path and short name, in any order', () => {
    expect(searchRepos(all, 'staged block')).toEqual([staged]);
    expect(searchRepos(all, 'BLOCK apps')).toEqual([staged, differ]);
    expect(searchRepos(all, 'staged cash')).toEqual([]);
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

describe('sidebarProjectMatches', () => {
  const alpha = project({ id: 'p1', name: 'Alpha Service' });
  const beta = project({ id: 'p2', name: 'beta-tools' });
  const projects = [alpha, beta];
  const ids = (query: string, snapshot?: SearchSnapshot) => {
    const rank = sidebarProjectMatches(projects, query, snapshot).rank;
    return rank && new Set(rank.keys());
  };

  it('is null for an empty or whitespace query, so every row shows', () => {
    expect(ids('')).toBeNull();
    expect(ids('  ')).toBeNull();
  });

  it('ranks the ids searchProjects keeps', () => {
    expect(ids('ALPHA')).toEqual(new Set(['p1']));
    expect(ids('a')).toEqual(new Set(['p1', 'p2']));
  });

  it('is an empty set, not null, when a query matches nothing', () => {
    expect(ids('zzz')).toEqual(new Set());
  });

  it('matches branches and PR numbers through a search snapshot', () => {
    const snapshot = buildSearchSnapshot(projects, {
      branchesByProject: new Map([
        ['p2', [{ branchName: 'fix-login', prNumber: 123, updatedAt: 0 } as Branch]],
      ]),
      reposByProject: new Map(),
    });
    expect(ids('login', snapshot)).toEqual(new Set(['p2']));
    expect(ids('#123', snapshot)).toEqual(new Set(['p2']));
    expect(ids('12', snapshot)).toEqual(new Set());
    expect(ids('login')).toEqual(new Set());
  });

  it('gives the reasons from the same pass', () => {
    const snapshot = buildSearchSnapshot(projects, {
      branchesByProject: new Map([
        ['p2', [{ branchName: 'fix-login', prNumber: null, updatedAt: 0 } as Branch]],
      ]),
      reposByProject: new Map(),
    });
    expect(sidebarProjectMatches(projects, 'login', snapshot).reasons).toEqual(
      new Map([['p2', { kind: 'branch', label: 'fix-login', more: 0, itemTerms: ['login'] }]])
    );
    expect(sidebarProjectMatches(projects, 'beta', snapshot).reasons.size).toBe(0);
  });

  // Rows show no tag chips, so a tag that matched says so on the meta line.
  it('matches a tag name, giving the tag as the reason', () => {
    const snapshot = buildSearchSnapshot(projects, {
      branchesByProject: new Map(),
      reposByProject: new Map(),
      tagNames: (projectId) => (projectId === 'p1' ? ['q4-launch'] : []),
    });
    expect(ids('q4', snapshot)).toEqual(new Set(['p1']));
    expect(sidebarProjectMatches(projects, 'q4', snapshot).reasons).toEqual(
      new Map([['p1', { kind: 'tag', label: 'q4-launch', more: 0, itemTerms: ['q4'] }]])
    );
  });

  it('matches terms spread across branches', () => {
    const snapshot = buildSearchSnapshot(projects, {
      branchesByProject: new Map([
        [
          'p1',
          [
            { branchName: 'fix-login', prNumber: null, updatedAt: 0 } as Branch,
            { branchName: 'tidy-typos', prNumber: null, updatedAt: 1 } as Branch,
          ],
        ],
      ]),
      reposByProject: new Map(),
    });
    expect(ids('login typos', snapshot)).toEqual(new Set(['p1']));
  });

  it('ranks rows best first, ties in row order, for the rows’ CSS order', () => {
    const gamma = project({ id: 'p3', name: 'Gamma' });
    const rows = [alpha, beta, gamma];
    const snapshot = buildSearchSnapshot(rows, {
      branchesByProject: new Map([
        ['p1', [{ branchName: 'tools-upgrade', prNumber: null, updatedAt: 0 } as Branch]],
        ['p3', [{ branchName: 'tools-cleanup', prNumber: null, updatedAt: 0 } as Branch]],
      ]),
      reposByProject: new Map(),
    });
    // beta has "tools" in its name; alpha and gamma only in a branch.
    expect(sidebarProjectMatches(rows, 'tools', snapshot).rank).toEqual(
      new Map([
        ['p2', 0],
        ['p1', 1],
        ['p3', 2],
      ])
    );
  });

  it('has no rank for a blank query, so rows keep their own order', () => {
    expect(sidebarProjectMatches(projects, '').rank).toBeNull();
  });
});

describe('resultsLabel', () => {
  it('pluralises everything but one', () => {
    expect(resultsLabel(0)).toBe('0 results');
    expect(resultsLabel(1)).toBe('1 result');
    expect(resultsLabel(2)).toBe('2 results');
  });
});
