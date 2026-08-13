import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectRepo } from '../../types';

// ── Fixtures ──

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

function projectRepo(overrides: Partial<ProjectRepo> = {}): ProjectRepo {
  return {
    id: 'r1',
    projectId: 'p1',
    githubRepo: 'org/alpha',
    branchName: 'feature',
    subpath: null,
    isPrimary: true,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

const never = () => false;

// ── Mock plumbing ──
//
// Only the pure helpers are under test, but importing the module also
// instantiates the store singleton, which pulls in the global data stores —
// mock those out and stub $state the way projectsData.test.ts does.

async function importHelpers() {
  return await import('./projectFilters.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // The store's runes compile away in the app build; under vitest they stay
  // plain global calls, so stub $state as identity (projectsData.test.ts
  // precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  vi.doMock('../../stores/projectsData.svelte', () => ({ projectsDataStore: {} }));
  vi.doMock('../../stores/projectState.svelte', () => ({ projectStateStore: {} }));
  vi.doMock('./projectStatus', () => ({ getProjectStatus: vi.fn() }));
});

afterEach(() => {
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('./projectStatus');
  vi.unstubAllGlobals();
});

// ── Tests ──

describe('filterKey / parseRepoFilterKey', () => {
  it('passes status filters through and formats repo filters', async () => {
    const { filterKey } = await importHelpers();
    expect(filterKey('unread')).toBe('unread');
    expect(filterKey('running')).toBe('running');
    expect(filterKey({ repo: 'org/alpha', subpath: '' })).toBe('repo:org/alpha:');
    expect(filterKey({ repo: 'org/alpha', subpath: 'apps/web' })).toBe('repo:org/alpha:apps/web');
  });

  it('parses repo keys back into their parts and rejects status keys', async () => {
    const { parseRepoFilterKey } = await importHelpers();
    expect(parseRepoFilterKey('repo:org/alpha:apps/web')).toEqual({
      repo: 'org/alpha',
      subpath: 'apps/web',
    });
    expect(parseRepoFilterKey('repo:org/alpha:')).toEqual({ repo: 'org/alpha', subpath: '' });
    expect(parseRepoFilterKey('unread')).toBeNull();
    expect(parseRepoFilterKey('running')).toBeNull();
  });

  it('round-trips repo filters, including subpaths containing a colon', async () => {
    const { filterKey, parseRepoFilterKey } = await importHelpers();
    const filter = { repo: 'org/alpha', subpath: 'apps:odd' };
    expect(parseRepoFilterKey(filterKey(filter))).toEqual(filter);
  });
});

describe('computeRepoFilters', () => {
  it('counts unique repo+subpath entries across hydrated projects', async () => {
    const { computeRepoFilters } = await importHelpers();
    const projects = [project({ id: 'p1' }), project({ id: 'p2' }), project({ id: 'p3' })];
    const repos = new Map([
      ['p1', [projectRepo({ githubRepo: 'org/alpha' })]],
      ['p2', [projectRepo({ githubRepo: 'org/alpha' }), projectRepo({ githubRepo: 'org/beta' })]],
      ['p3', [projectRepo({ githubRepo: 'org/alpha', subpath: 'apps/web' })]],
    ]);

    expect(computeRepoFilters(projects, repos)).toEqual([
      { repo: 'org/alpha', subpath: '', count: 2 },
      { repo: 'org/alpha', subpath: 'apps/web', count: 1 },
      { repo: 'org/beta', subpath: '', count: 1 },
    ]);
  });

  it('prefers headRepo over githubRepo for display', async () => {
    const { computeRepoFilters } = await importHelpers();
    const repos = new Map([
      ['p1', [projectRepo({ githubRepo: 'org/alpha', headRepo: 'fork/alpha' })]],
    ]);

    expect(computeRepoFilters([project()], repos)).toEqual([
      { repo: 'fork/alpha', subpath: '', count: 1 },
    ]);
  });

  it('falls back to the legacy project.githubRepo only when the repos list is empty', async () => {
    const { computeRepoFilters } = await importHelpers();
    const projects = [
      project({ id: 'p1', githubRepo: 'org/legacy', subpath: 'sub' }),
      project({ id: 'p2', githubRepo: 'org/ignored' }),
    ];
    const repos = new Map([['p2', [projectRepo({ githubRepo: 'org/hydrated' })]]]);

    expect(computeRepoFilters(projects, repos)).toEqual([
      { repo: 'org/hydrated', subpath: '', count: 1 },
      { repo: 'org/legacy', subpath: 'sub', count: 1 },
    ]);
  });

  it('skips projects with no repos and no githubRepo', async () => {
    const { computeRepoFilters } = await importHelpers();
    expect(computeRepoFilters([project({ githubRepo: null })], new Map())).toEqual([]);
  });

  it('sorts by full display string including subpath', async () => {
    const { computeRepoFilters } = await importHelpers();
    const projects = [project({ id: 'p1' }), project({ id: 'p2' }), project({ id: 'p3' })];
    const repos = new Map([
      ['p1', [projectRepo({ githubRepo: 'org/b' })]],
      ['p2', [projectRepo({ githubRepo: 'org/a', subpath: 'z' })]],
      ['p3', [projectRepo({ githubRepo: 'org/a', subpath: 'm' })]],
    ]);

    expect(computeRepoFilters(projects, repos).map((rf) => `${rf.repo}:${rf.subpath}`)).toEqual([
      'org/a:m',
      'org/a:z',
      'org/b:',
    ]);
  });
});

describe('filterProjects', () => {
  it('returns every project when no filters are active', async () => {
    const { filterProjects } = await importHelpers();
    const projects = [project({ id: 'p1' }), project({ id: 'p2' })];
    expect(filterProjects(projects, new Set(), new Map(), never, never)).toBe(projects);
  });

  it('ANDs status filters with each other', async () => {
    const { filterProjects } = await importHelpers();
    const projects = [project({ id: 'p1' }), project({ id: 'p2' }), project({ id: 'p3' })];
    const isUnread = (id: string) => id !== 'p3';
    const isRunning = (id: string) => id !== 'p1';

    expect(
      filterProjects(projects, new Set(['unread']), new Map(), isUnread, isRunning).map((p) => p.id)
    ).toEqual(['p1', 'p2']);
    expect(
      filterProjects(projects, new Set(['unread', 'running']), new Map(), isUnread, isRunning).map(
        (p) => p.id
      )
    ).toEqual(['p2']);
  });

  it('ORs repo filters with each other and ANDs them with status filters', async () => {
    const { filterProjects } = await importHelpers();
    const projects = [project({ id: 'p1' }), project({ id: 'p2' }), project({ id: 'p3' })];
    const repos = new Map([
      ['p1', [projectRepo({ githubRepo: 'org/alpha' })]],
      ['p2', [projectRepo({ githubRepo: 'org/beta' })]],
      ['p3', [projectRepo({ githubRepo: 'org/gamma' })]],
    ]);
    const bothRepos = new Set(['repo:org/alpha:', 'repo:org/beta:']);

    expect(filterProjects(projects, bothRepos, repos, never, never).map((p) => p.id)).toEqual([
      'p1',
      'p2',
    ]);
    expect(
      filterProjects(
        projects,
        new Set(['unread', 'repo:org/alpha:', 'repo:org/beta:']),
        repos,
        (id) => id === 'p2',
        never
      ).map((p) => p.id)
    ).toEqual(['p2']);
  });

  it('matches repo filters against headRepo when present', async () => {
    const { filterProjects } = await importHelpers();
    const projects = [project({ id: 'p1' })];
    const repos = new Map([
      ['p1', [projectRepo({ githubRepo: 'org/alpha', headRepo: 'fork/alpha' })]],
    ]);

    expect(
      filterProjects(projects, new Set(['repo:fork/alpha:']), repos, never, never)
    ).toHaveLength(1);
    expect(
      filterProjects(projects, new Set(['repo:org/alpha:']), repos, never, never)
    ).toHaveLength(0);
  });

  it('falls back to the legacy project.githubRepo when the repos list is empty', async () => {
    const { filterProjects } = await importHelpers();
    const projects = [
      project({ id: 'p1', githubRepo: 'org/legacy', subpath: 'sub' }),
      project({ id: 'p2', githubRepo: null }),
    ];

    expect(
      filterProjects(projects, new Set(['repo:org/legacy:sub']), new Map(), never, never).map(
        (p) => p.id
      )
    ).toEqual(['p1']);
  });

  it('excludes repo-less projects when a repo filter is active', async () => {
    const { filterProjects } = await importHelpers();
    const projects = [project({ id: 'p1', githubRepo: null })];
    expect(filterProjects(projects, new Set(['repo:org/alpha:']), new Map(), never, never)).toEqual(
      []
    );
  });
});

describe('toggleFilterKey', () => {
  it('plain click selects the filter exclusively', async () => {
    const { toggleFilterKey } = await importHelpers();
    expect(toggleFilterKey(new Set(), 'unread', false)).toEqual(new Set(['unread']));
    expect(toggleFilterKey(new Set(['running', 'repo:org/alpha:']), 'unread', false)).toEqual(
      new Set(['unread'])
    );
  });

  it('plain click on an active filter among others collapses to just it', async () => {
    const { toggleFilterKey } = await importHelpers();
    expect(toggleFilterKey(new Set(['unread', 'running']), 'unread', false)).toEqual(
      new Set(['unread'])
    );
  });

  it('plain click on the only active filter deselects it', async () => {
    const { toggleFilterKey } = await importHelpers();
    expect(toggleFilterKey(new Set(['unread']), 'unread', false)).toEqual(new Set());
  });

  it('shift-click toggles the filter within the current set', async () => {
    const { toggleFilterKey } = await importHelpers();
    expect(toggleFilterKey(new Set(['unread']), 'running', true)).toEqual(
      new Set(['unread', 'running'])
    );
    expect(toggleFilterKey(new Set(['unread', 'running']), 'running', true)).toEqual(
      new Set(['unread'])
    );
  });

  it('returns a new Set rather than mutating the input', async () => {
    const { toggleFilterKey } = await importHelpers();
    const input = new Set(['unread']);
    const next = toggleFilterKey(input, 'running', true);
    expect(next).not.toBe(input);
    expect(input).toEqual(new Set(['unread']));
  });
});

describe('hasRepoFilterKeys', () => {
  it('is true only when a non-status key is active', async () => {
    const { hasRepoFilterKeys } = await importHelpers();
    expect(hasRepoFilterKeys(new Set())).toBe(false);
    expect(hasRepoFilterKeys(new Set(['unread', 'running']))).toBe(false);
    expect(hasRepoFilterKeys(new Set(['unread', 'repo:org/alpha:']))).toBe(true);
  });
});
