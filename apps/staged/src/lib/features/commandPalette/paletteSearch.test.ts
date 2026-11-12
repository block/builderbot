import { describe, expect, it } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../../types';
import type { HighlightSegment } from '../../shared/textSearch';
import { buildSearchSnapshot } from '../projects/projectSearch';
import {
  PALETTE_ROW_LIMIT,
  byRecentActivity,
  fuzzyHighlightSegments,
  fuzzyMatch,
  hiddenProjectIds,
  moveSelection,
  paletteResults,
  projectActivityAt,
  projectRepoPaths,
  projectRepos,
  type PaletteSearchInput,
  type RepoShortName,
} from './paletteSearch';

// ── Fixtures ──

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p1',
    name: 'Alpha',
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: null,
    branchName: 'main-work',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: null,
    createdAt: 0,
    updatedAt: 0,
    prState: null,
    prChecksStatus: null,
    prReviewDecision: null,
    prMergeable: null,
    prDraft: null,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    prTitle: null,
    ...overrides,
  } as Branch;
}

function repo(overrides: Partial<ProjectRepo> = {}): ProjectRepo {
  return {
    id: 'r1',
    projectId: 'p1',
    githubRepo: 'org/alpha',
    branchName: 'main',
    subpath: null,
    isPrimary: true,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

interface World {
  projects: Project[];
  branches?: Record<string, Branch[]>;
  repos?: Record<string, ProjectRepo[]>;
  /** Badge short names by repo path (`owner/name` or `owner/name/subpath`). */
  shortNames?: Record<string, string>;
}

/** paletteResults over a world, with the search snapshot the palette builds. */
function search(world: World, query: string, extra: Partial<PaletteSearchInput> = {}) {
  const branchesByProject = new Map(Object.entries(world.branches ?? {}));
  const reposByProject = new Map(Object.entries(world.repos ?? {}));
  const repoShortName: RepoShortName = (githubRepo, subpath) =>
    world.shortNames?.[subpath ? `${githubRepo}/${subpath}` : githubRepo];
  return paletteResults({
    projects: world.projects,
    query,
    snapshot: buildSearchSnapshot(world.projects, {
      branchesByProject,
      reposByProject,
      repoShortName,
    }),
    branchesByProject,
    reposByProject,
    repoShortName,
    ...extra,
  });
}

const names = (results: { rows: Array<{ project: Project }> }) =>
  results.rows.map((row) => row.project.name);

/** A row's highlighted text, matches in brackets: `[Com]mand`. */
const marked = (segments: readonly HighlightSegment[] | null) =>
  (segments ?? []).map((s) => (s.match ? `[${s.text}]` : s.text)).join('');

// ── fuzzyMatch ──

describe('fuzzyMatch', () => {
  it('finds the term as an in-order subsequence and says where', () => {
    expect(fuzzyMatch('cmdk', 'Command K search')?.positions).toEqual([0, 2, 6, 8]);
    expect(fuzzyMatch('stgd', 'staged')?.positions).toEqual([0, 1, 3, 5]);
  });

  it('is case-insensitive', () => {
    expect(fuzzyMatch('cmdk', 'COMMAND K')).not.toBeNull();
  });

  it('prefers characters that start a word', () => {
    // The `s` of "search", not the earlier one inside "sessions".
    expect(fuzzyMatch('cks', 'Command K sessions search')?.positions).toEqual([0, 8, 10]);
    expect(fuzzyMatch('np', 'newProject')?.positions).toEqual([0, 3]);
    // Of two word starts, the one that skips less: `builderbot`, not `block`.
    expect(fuzzyMatch('bas', 'block/builderbot/apps/staged')?.positions).toEqual([6, 17, 22]);
  });

  it('prefers a run of adjacent characters to a scattered match', () => {
    expect(fuzzyMatch('bot', 'builder about bot')?.positions).toEqual([14, 15, 16]);
  });

  it('must start at the beginning of a word', () => {
    expect(fuzzyMatch('in', 'main')).toBeNull();
    expect(fuzzyMatch('lder', 'builder')).toBeNull();
  });

  it('rejects a match that skips more than it earns', () => {
    expect(fuzzyMatch('ab', 'a very long name with b')).toBeNull();
    expect(fuzzyMatch('bb', 'builderbot')).toBeNull();
  });

  it('rejects characters that are out of order or missing', () => {
    expect(fuzzyMatch('kc', 'Command K')).toBeNull();
    expect(fuzzyMatch('cmdx', 'Command K')).toBeNull();
  });

  it('leaves single characters and over-long terms to the substring match', () => {
    expect(fuzzyMatch('c', 'Command')).toBeNull();
    expect(fuzzyMatch('commands', 'Command')).toBeNull();
  });

  it('scores a tighter match higher', () => {
    const tight = fuzzyMatch('stg', 'staged')!;
    const loose = fuzzyMatch('stg', 'some tall grass')!;
    expect(tight.score).toBeGreaterThan(fuzzyMatch('sgd', 'staged')!.score);
    expect(loose.score).toBeGreaterThan(0);
  });

  it('skips a text whose lower-casing changes its length', () => {
    expect(fuzzyMatch('ist', 'İstanbul trip')).toBeNull();
  });
});

describe('fuzzyHighlightSegments', () => {
  it('marks every occurrence of a term the text holds as a substring', () => {
    expect(marked(fuzzyHighlightSegments('Alpha alpha', ['alp']))).toBe('[Alp]ha [alp]ha');
  });

  it('marks the matched characters of a fuzzy term', () => {
    expect(marked(fuzzyHighlightSegments('Command K search', ['cmdk']))).toBe(
      '[C]o[m]man[d] [K] search'
    );
  });

  it('marks a mix of both, and leaves an unmatched term alone', () => {
    expect(marked(fuzzyHighlightSegments('Command K search', ['sea', 'cmdk', 'zzz']))).toBe(
      '[C]o[m]man[d] [K] [sea]rch'
    );
  });

  it('falls back to substring highlighting when lower-casing changes the length', () => {
    expect(marked(fuzzyHighlightSegments('İstanbul trip', ['trip']))).toBe('İstanbul [trip]');
  });
});

// ── Recency ──

describe('recency', () => {
  it('takes the latest of the project and its branches', () => {
    const p = project({ createdAt: 5, updatedAt: 10 });
    expect(projectActivityAt(p)).toBe(10);
    expect(projectActivityAt(p, [branch({ updatedAt: 7 }), branch({ updatedAt: 30 })])).toBe(30);
    expect(projectActivityAt(project({ createdAt: 9, updatedAt: 2 }))).toBe(9);
  });

  it('orders most recently active first, ties in input order', () => {
    const old = project({ id: 'old', updatedAt: 1 });
    const tieA = project({ id: 'tie-a', updatedAt: 5 });
    const tieB = project({ id: 'tie-b', updatedAt: 5 });
    const busy = project({ id: 'busy', updatedAt: 2 });
    const branches = new Map([['busy', [branch({ projectId: 'busy', updatedAt: 9 })]]]);
    expect(byRecentActivity([old, tieA, tieB, busy], branches).map((p) => p.id)).toEqual([
      'busy',
      'tie-a',
      'tie-b',
      'old',
    ]);
  });
});

describe('projectRepoPaths', () => {
  it('lists hydrated repos primary first, with subpaths and fork heads', () => {
    const p = project({ githubRepo: 'org/legacy' });
    expect(
      projectRepoPaths(p, [
        repo({ id: 'r2', githubRepo: 'org/tools', isPrimary: false }),
        repo({ id: 'r1', githubRepo: 'org/mono', subpath: 'apps/web' }),
        repo({ id: 'r3', githubRepo: 'org/base', headRepo: 'me/fork', isPrimary: false }),
      ])
    ).toEqual(['org/mono/apps/web', 'org/tools', 'me/fork']);
  });

  it("falls back to the project's own repo until its repos hydrate", () => {
    expect(projectRepoPaths(project({ githubRepo: 'org/a', subpath: 'pkg' }), undefined)).toEqual([
      'org/a/pkg',
    ]);
    expect(projectRepoPaths(project(), [])).toEqual([]);
  });
});

describe('projectRepos', () => {
  const shortName: RepoShortName = (githubRepo, subpath) =>
    githubRepo === 'org/base' && subpath === null ? 'core' : undefined;

  it('keeps the base path and the short name a fork head stands in front of', () => {
    expect(
      projectRepos(
        project(),
        [
          repo({ id: 'r1', githubRepo: 'org/mono', subpath: 'apps/web' }),
          repo({ id: 'r2', githubRepo: 'org/base', headRepo: 'me/fork', isPrimary: false }),
        ],
        shortName
      )
    ).toEqual([
      { path: 'org/mono/apps/web', basePath: null, shortName: null },
      // The short name belongs to the base repo, which is what is badged.
      { path: 'me/fork', basePath: 'org/base', shortName: 'core' },
    ]);
  });

  it("labels the project's own repo until its repos hydrate", () => {
    expect(projectRepos(project({ githubRepo: 'org/base' }), undefined, shortName)).toEqual([
      { path: 'org/base', basePath: null, shortName: 'core' },
    ]);
  });
});

// ── paletteResults ──

describe('paletteResults with no query', () => {
  const stale = project({ id: 'stale', name: 'Stale', updatedAt: 1 });
  const recent = project({ id: 'recent', name: 'Recent', updatedAt: 50 });
  const busy = project({ id: 'busy', name: 'Busy', updatedAt: 2 });
  const world: World = {
    projects: [stale, recent, busy],
    branches: { busy: [branch({ projectId: 'busy', updatedAt: 99 })] },
  };

  it('lists every project, most recently active first', () => {
    const results = search(world, '');
    expect(names(results)).toEqual(['Busy', 'Recent', 'Stale']);
    expect(results.total).toBe(3);
    expect(results.terms).toEqual([]);
  });

  it('treats a whitespace-only query as none', () => {
    expect(names(search(world, '   '))).toEqual(['Busy', 'Recent', 'Stale']);
  });

  it('lists the project on screen last', () => {
    expect(names(search(world, '', { currentProjectId: 'busy' }))).toEqual([
      'Recent',
      'Stale',
      'Busy',
    ]);
  });

  it('highlights nothing and shows the primary repo', () => {
    const [row] = search(
      {
        projects: [project({ id: 'p1', name: 'Alpha' })],
        repos: {
          p1: [repo({ id: 'r2', githubRepo: 'org/second', isPrimary: false }), repo({ id: 'r1' })],
        },
      },
      ''
    ).rows;
    expect(marked(row.name)).toBe('Alpha');
    expect(marked(row.repo)).toBe('org/alpha');
    expect(row.moreRepos).toBe(1);
    expect(row.reason).toBeNull();
  });

  it('caps the rows and still reports how many there are', () => {
    const many = Array.from({ length: PALETTE_ROW_LIMIT + 5 }, (_, i) =>
      project({ id: `p${i}`, name: `Project ${i}` })
    );
    const results = search({ projects: many }, '');
    expect(results.rows).toHaveLength(PALETTE_ROW_LIMIT);
    expect(results.total).toBe(PALETTE_ROW_LIMIT + 5);
    expect(search({ projects: many }, '', { limit: 3 }).rows).toHaveLength(3);
  });
});

describe('paletteResults with a query', () => {
  const checkout = project({ id: 'checkout', name: 'Checkout flow', updatedAt: 3 });
  const palette = project({ id: 'palette', name: 'Command K search', updatedAt: 2 });
  const docs = project({ id: 'docs', name: 'Docs refresh', updatedAt: 1 });
  const world: World = {
    projects: [checkout, palette, docs],
    repos: {
      checkout: [repo({ projectId: 'checkout', githubRepo: 'block/payments' })],
      palette: [
        repo({ projectId: 'palette', githubRepo: 'block/builderbot', subpath: 'apps/staged' }),
        repo({ id: 'r9', projectId: 'palette', githubRepo: 'block/search-kit', isPrimary: false }),
      ],
      docs: [repo({ projectId: 'docs', githubRepo: 'block/handbook' })],
    },
    branches: {
      docs: [branch({ projectId: 'docs', branchName: 'matt/checkout-guide', updatedAt: 1 })],
    },
  };

  it('matches the name case-insensitively and highlights the match', () => {
    const results = search(world, 'COMMAND');
    expect(names(results)).toEqual(['Command K search']);
    expect(marked(results.rows[0].name)).toBe('[Command] K search');
    expect(results.terms).toEqual(['command']);
  });

  it('matches every term, in any order, across fields', () => {
    expect(names(search(world, 'search command'))).toEqual(['Command K search']);
    // One term in the name, the other in the repo path.
    const results = search(world, 'flow payments');
    expect(names(results)).toEqual(['Checkout flow']);
    expect(marked(results.rows[0].name)).toBe('Checkout [flow]');
    expect(marked(results.rows[0].repo)).toBe('block/[payments]');
  });

  it('matches a repo slug and highlights it in the repo line', () => {
    const results = search(world, 'handbook');
    expect(names(results)).toEqual(['Docs refresh']);
    expect(marked(results.rows[0].name)).toBe('Docs refresh');
    expect(marked(results.rows[0].repo)).toBe('block/[handbook]');
  });

  it('shows the repo the query matched, not just the primary', () => {
    const [row] = search(world, 'search-kit').rows;
    expect(marked(row.repo)).toBe('block/[search-kit]');
    expect(row.moreRepos).toBe(1);
  });

  // What the app search matches a repo on beyond the path the row shows: a
  // project found only that way must still say why.
  describe('found by a repo label the row does not show by default', () => {
    const forked = project({ id: 'forked', name: 'Retry backoff' });
    const badged = project({ id: 'badged', name: 'Ledger export' });
    const labels: World = {
      projects: [forked, badged],
      repos: {
        forked: [
          repo({ projectId: 'forked', githubRepo: 'block/payments', headRepo: 'matt/pay-fork' }),
        ],
        badged: [
          repo({ projectId: 'badged', githubRepo: 'block/ledger', subpath: 'tools/export' }),
          repo({ id: 'r2', projectId: 'badged', githubRepo: 'block/infra', isPrimary: false }),
        ],
      },
      shortNames: { 'block/ledger/tools/export': 'books', 'block/infra': 'platform' },
    };

    it("shows the head repo of a fork PR when the query doesn't say otherwise", () => {
      const [row] = search(labels, 'retry').rows;
      expect(marked(row.repo)).toBe('matt/pay-fork');
      expect(row.repoAlias).toBeNull();
    });

    it('shows the base repo path when that is what the query matched', () => {
      const results = search(labels, 'payments');
      expect(names(results)).toEqual(['Retry backoff']);
      expect(marked(results.rows[0].repo)).toBe('block/[payments]');
      expect(results.rows[0].repoAlias).toBeNull();
    });

    it('still prefers the head repo path when the query matches both', () => {
      const [row] = search(labels, 'pay').rows;
      expect(marked(row.repo)).toBe('matt/[pay]-fork');
    });

    it('shows the badge short name beside the path when that is what matched', () => {
      const results = search(labels, 'books');
      expect(names(results)).toEqual(['Ledger export']);
      expect(marked(results.rows[0].repoAlias)).toBe('[books]');
      expect(marked(results.rows[0].repo)).toBe('block/ledger/tools/export');
    });

    it("shows a secondary repo's short name with that repo's path", () => {
      const [row] = search(labels, 'platform').rows;
      expect(marked(row.repoAlias)).toBe('[platform]');
      expect(marked(row.repo)).toBe('block/infra');
      expect(row.moreRepos).toBe(1);
    });

    it('leaves the short name out when the path already shows the match', () => {
      const [row] = search(labels, 'ledger').rows;
      expect(marked(row.repo)).toBe('block/[ledger]/tools/export');
      expect(row.repoAlias).toBeNull();
    });
  });

  it('matches a branch name and says so in the reason', () => {
    const results = search(world, 'guide');
    expect(names(results)).toEqual(['Docs refresh']);
    expect(results.rows[0].reason).toMatchObject({ kind: 'branch', label: 'matt/checkout-guide' });
  });

  it('ranks a name match above a branch match', () => {
    expect(names(search(world, 'checkout'))).toEqual(['Checkout flow', 'Docs refresh']);
  });

  it('keeps equally good matches most recently active first', () => {
    const older = project({ id: 'older', name: 'Search older', updatedAt: 1 });
    const newer = project({ id: 'newer', name: 'Search newer', updatedAt: 9 });
    expect(names(search({ projects: [older, newer] }, 'search'))).toEqual([
      'Search newer',
      'Search older',
    ]);
  });

  it('falls back to a fuzzy match when no substring matches', () => {
    const results = search(world, 'cmdk');
    expect(names(results)).toEqual(['Command K search']);
    expect(marked(results.rows[0].name)).toBe('[C]o[m]man[d] [K] search');
  });

  it('matches a repo path fuzzily', () => {
    const results = search(world, 'bldbot');
    expect(names(results)).toEqual(['Command K search']);
    expect(marked(results.rows[0].repo)).toBe('block/[b]ui[ld]er[bot]/apps/staged');
  });

  it('mixes substring and fuzzy terms in one query', () => {
    expect(names(search(world, 'staged cmdk'))).toEqual(['Command K search']);
    expect(names(search(world, 'payments cmdk'))).toEqual([]);
  });

  it('ranks every substring match above every fuzzy one', () => {
    const exact = project({ id: 'exact', name: 'Notes on docs', updatedAt: 0 });
    const fuzzy = project({ id: 'fuzzy', name: 'Deploy on cloud staging', updatedAt: 99 });
    const fuzzier = project({ id: 'fuzzier', name: 'Design our component set', updatedAt: 50 });
    const results = search({ projects: [fuzzy, fuzzier, exact] }, 'docs');
    expect(names(results)).toEqual([
      'Notes on docs',
      'Deploy on cloud staging',
      'Design our component set',
    ]);
    expect(results.total).toBe(3);
  });

  it('ranks a fuzzy name match above a fuzzy repo match', () => {
    const byRepo = project({ id: 'by-repo', name: 'Zeta', githubRepo: 'org/command-kit' });
    const byName = project({ id: 'by-name', name: 'Command K' });
    expect(names(search({ projects: [byRepo, byName] }, 'cmdk'))).toEqual(['Command K', 'Zeta']);
  });

  it('returns nothing when nothing matches', () => {
    const results = search(world, 'zzzz');
    expect(results.rows).toEqual([]);
    expect(results.total).toBe(0);
  });

  it('searches names alone without a snapshot', () => {
    const results = paletteResults({ projects: world.projects, query: 'checkout' });
    expect(names(results)).toEqual(['Checkout flow']);
  });

  it('caps the rows across both tiers', () => {
    const many = Array.from({ length: 6 }, (_, i) => project({ id: `p${i}`, name: `Docs ${i}` }));
    const results = search({ projects: many }, 'docs', { limit: 4 });
    expect(results.rows).toHaveLength(4);
    expect(results.total).toBe(6);
  });
});

// ── hiddenProjectIds ──

describe('hiddenProjectIds', () => {
  const alpha = project({ id: 'alpha', name: 'Alpha service' });
  const beta = project({ id: 'beta', name: 'Beta tools' });
  const gamma = project({ id: 'gamma', name: 'Gamma web' });
  const projects = [alpha, beta, gamma];
  const hidden = (list: Partial<Parameters<typeof hiddenProjectIds>[0]>) =>
    [
      ...hiddenProjectIds({ projects, chipFiltered: projects, stickyId: null, query: '', ...list }),
    ].sort();

  it('hides nothing while the list shows everything', () => {
    expect(hidden({})).toEqual([]);
  });

  it('hides what the chip filters exclude', () => {
    expect(hidden({ chipFiltered: [alpha] })).toEqual(['beta', 'gamma']);
  });

  it('does not hide the sticky project the list keeps despite the chips', () => {
    // The sidebar's selected row, or the card the grid came back to.
    expect(hidden({ chipFiltered: [alpha], stickyId: 'gamma' })).toEqual(['beta']);
    expect(hidden({ chipFiltered: [], stickyId: 'beta' })).toEqual(['alpha', 'gamma']);
  });

  it('hides what the search field rules out, the sticky project included', () => {
    expect(hidden({ query: 'beta' })).toEqual(['alpha', 'gamma']);
    // A typed query is a deliberate narrowing: neither list exempts its
    // sticky project from it.
    expect(hidden({ chipFiltered: [alpha], stickyId: 'gamma', query: 'alpha' })).toEqual([
      'beta',
      'gamma',
    ]);
    expect(hidden({ chipFiltered: [alpha], stickyId: 'gamma', query: 'web' })).toEqual([
      'alpha',
      'beta',
    ]);
  });

  it('matches the query on what the snapshot holds, like the lists do', () => {
    const branchesByProject = new Map([
      ['gamma', [branch({ projectId: 'gamma', branchName: 'matt/dark-mode' })]],
    ]);
    const snapshot = buildSearchSnapshot(projects, {
      branchesByProject,
      reposByProject: new Map(),
    });
    expect(hidden({ query: 'dark-mode', snapshot })).toEqual(['alpha', 'beta']);
  });

  it('ignores a sticky project that is not in the list at all', () => {
    expect(hidden({ chipFiltered: [alpha], stickyId: 'gone' })).toEqual(['beta', 'gamma']);
  });
});

// ── moveSelection ──

describe('moveSelection', () => {
  it('moves one row at a time', () => {
    expect(moveSelection(0, 1, 3)).toBe(1);
    expect(moveSelection(2, -1, 3)).toBe(1);
  });

  it('wraps at both ends', () => {
    expect(moveSelection(2, 1, 3)).toBe(0);
    expect(moveSelection(0, -1, 3)).toBe(2);
  });

  it('stays put in a one-row list and has nothing to pick in an empty one', () => {
    expect(moveSelection(0, 1, 1)).toBe(0);
    expect(moveSelection(0, -1, 1)).toBe(0);
    expect(moveSelection(0, 1, 0)).toBe(-1);
  });

  it('moves from the nearest end when the index is out of range', () => {
    expect(moveSelection(-1, 1, 3)).toBe(1);
    expect(moveSelection(9, 1, 3)).toBe(0);
  });
});
