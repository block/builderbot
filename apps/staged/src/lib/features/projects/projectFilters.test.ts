import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectRepo, TagState } from '../../types';
import { indexAssignments, indexTagCatalog } from '../tags/tagState';
import { tag, tagState } from '../tags/testStubs/tagFixtures';
import type { TagFilterContext } from './projectFilters.svelte';

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

/** The tag filters' context over a snapshot, as the store builds it from tagsStore. */
function tagContext(state: TagState): TagFilterContext {
  const catalog = indexTagCatalog(state);
  const byProject = indexAssignments(state, catalog.tagsById);
  return {
    tagIdsFor: (projectId) => byProject.get(projectId) ?? new Set(),
    groupOf: (tagId) => catalog.tagsById.get(tagId)?.groupId ?? null,
    knownTagIds: catalog.tagsById,
  };
}

/** The tagsStore mock's fields, swapped per test. */
const tagsMock = vi.hoisted(() => ({ tagsById: new Map<string, unknown>() }));

// ── Mock plumbing ──
//
// Only the pure helpers are under test, but importing the module also
// instantiates the store singleton, which pulls in the global data stores —
// mock those out and stub $state the way agent.test.ts does.

async function importHelpers() {
  return await import('./projectFilters.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // The store's runes compile away in the app build; under vitest they stay
  // plain global calls, so stub $state as identity (agent.test.ts
  // precedent). $derived needs the same treatment for the store's derived
  // fields, and since the stub evaluates them eagerly at construction the
  // mocked data stores have to carry the shape those fields read.
  vi.stubGlobal('$state', (initial: unknown) => initial);
  const derived = (value: unknown) => value;
  derived.by = (compute: () => unknown) => compute();
  vi.stubGlobal('$derived', derived);
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: { liveProjects: [], reposByProject: new Map() },
  }));
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: { isUnread: () => false },
  }));
  vi.doMock('./projectStatus', () => ({ getProjectStatus: vi.fn() }));
  vi.doMock('./workspaceLifecycle.svelte', () => ({
    workspaceLifecycle: { getWorktreeErrors: () => new Map() },
  }));
  tagsMock.tagsById = new Map();
  vi.doMock('../../stores/tags.svelte', () => ({
    tagsStore: {
      get tagsById() {
        return tagsMock.tagsById;
      },
      catalog: indexTagCatalog({ groups: [], tags: [], assignments: [] }),
      usageCount: new Map(),
      tagIdsFor: () => new Set(),
    },
  }));
});

afterEach(() => {
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('./projectStatus');
  vi.doUnmock('./workspaceLifecycle.svelte');
  vi.doUnmock('../../stores/tags.svelte');
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

describe('filterKey / parseTagFilterKey', () => {
  it('formats tag filters as tag:<id> and parses them back', async () => {
    const { filterKey, parseTagFilterKey } = await importHelpers();
    expect(filterKey({ tagId: 'q4' })).toBe('tag:q4');
    expect(parseTagFilterKey('tag:q4')).toEqual({ tagId: 'q4' });
    const uuid = { tagId: '6f1c2a9e-4b7d-4e0a-9c3b-1d2e3f4a5b6c' };
    expect(parseTagFilterKey(filterKey(uuid))).toEqual(uuid);
  });

  it('rejects every other key, and an empty id', async () => {
    const { parseTagFilterKey } = await importHelpers();
    expect(parseTagFilterKey('unread')).toBeNull();
    expect(parseTagFilterKey('repo:org/alpha:')).toBeNull();
    expect(parseTagFilterKey('tag:')).toBeNull();
  });

  it('never reads a tag key as a repo key, or a repo key as a tag key', async () => {
    const { hasRepoFilterKeys, parseRepoFilterKey, parseTagFilterKey } = await importHelpers();
    expect(parseRepoFilterKey('tag:q4')).toBeNull();
    // A repo whose name starts "tag" is still a repo.
    expect(parseTagFilterKey('repo:tag/x:')).toBeNull();
    expect(hasRepoFilterKeys(new Set(['tag:q4', 'tag:p0']))).toBe(false);
    expect(hasRepoFilterKeys(new Set(['tag:q4', 'repo:org/alpha:']))).toBe(true);
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
  it('is true only when a repo key is active', async () => {
    const { hasRepoFilterKeys } = await importHelpers();
    expect(hasRepoFilterKeys(new Set())).toBe(false);
    expect(hasRepoFilterKeys(new Set(['unread', 'running']))).toBe(false);
    expect(hasRepoFilterKeys(new Set(['unread', 'repo:org/alpha:']))).toBe(true);
  });

  it('does not treat an unrecognized status key as a repo filter', async () => {
    const { hasRepoFilterKeys, filterProjects } = await importHelpers();
    // A third status filter must not make every project fail the repo check.
    expect(hasRepoFilterKeys(new Set(['archived']))).toBe(false);
    const projects = [project({ id: 'p1' })];
    expect(filterProjects(projects, new Set(['archived']), new Map(), never, never)).toHaveLength(
      1
    );
  });
});

describe('filterProjects with tag filters', () => {
  // Status (icon group: idea, doing, done), Priority (p0, p1), ungrouped q4
  // and infra.
  //   a: doing, p0, q4        b: doing, p1         c: done, p0, q4, infra
  //   d: idea                 e: (no tags)
  const ids = ['a', 'b', 'c', 'd', 'e'];
  const projects = ids.map((id) => project({ id }));
  const state = tagState([
    { projectId: 'a', tagId: 'doing' },
    { projectId: 'a', tagId: 'p0' },
    { projectId: 'a', tagId: 'q4' },
    { projectId: 'b', tagId: 'doing' },
    { projectId: 'b', tagId: 'p1' },
    { projectId: 'c', tagId: 'done' },
    { projectId: 'c', tagId: 'p0' },
    { projectId: 'c', tagId: 'q4' },
    { projectId: 'c', tagId: 'infra' },
    { projectId: 'd', tagId: 'idea' },
  ]);
  const tags = tagContext(state);

  async function kept(
    keys: string[],
    {
      ctx = tags,
      repos = new Map<string, ProjectRepo[]>(),
      isUnread = never,
      isRunning = never,
    }: {
      ctx?: TagFilterContext;
      repos?: Map<string, ProjectRepo[]>;
      isUnread?: (id: string) => boolean;
      isRunning?: (id: string) => boolean;
    } = {}
  ) {
    const { filterProjects } = await importHelpers();
    return filterProjects(projects, new Set(keys), repos, isUnread, isRunning, ctx).map(
      (p) => p.id
    );
  }

  it('keeps the projects carrying a tag', async () => {
    expect(await kept(['tag:doing'])).toEqual(['a', 'b']);
    expect(await kept(['tag:q4'])).toEqual(['a', 'c']);
  });

  it('ORs tags in one group', async () => {
    expect(await kept(['tag:doing', 'tag:done'])).toEqual(['a', 'b', 'c']);
    expect(await kept(['tag:p0', 'tag:p1'])).toEqual(['a', 'b', 'c']);
  });

  it('ANDs one group with another', async () => {
    expect(await kept(['tag:doing', 'tag:p0'])).toEqual(['a']);
    expect(await kept(['tag:doing', 'tag:done', 'tag:p0'])).toEqual(['a', 'c']);
  });

  it('ANDs each ungrouped tag, with each other and with groups', async () => {
    expect(await kept(['tag:q4', 'tag:infra'])).toEqual(['c']);
    expect(await kept(['tag:q4', 'tag:doing'])).toEqual(['a']);
    expect(await kept(['tag:q4', 'tag:doing', 'tag:done', 'tag:p0'])).toEqual(['a', 'c']);
  });

  it('ANDs tags with unread and running', async () => {
    expect(await kept(['tag:doing', 'unread'], { isUnread: (id) => id === 'b' })).toEqual(['b']);
    expect(await kept(['tag:q4', 'running'], { isRunning: (id) => id !== 'a' })).toEqual(['c']);
  });

  it('ANDs tags with the repo filters, which still OR among themselves', async () => {
    const repos = new Map([
      ['a', [projectRepo({ githubRepo: 'org/alpha' })]],
      ['b', [projectRepo({ githubRepo: 'org/beta' })]],
      ['c', [projectRepo({ githubRepo: 'org/gamma' })]],
    ]);
    expect(await kept(['tag:doing', 'repo:org/beta:'], { repos })).toEqual(['b']);
    expect(await kept(['tag:p0', 'repo:org/alpha:', 'repo:org/gamma:'], { repos })).toEqual([
      'a',
      'c',
    ]);
  });

  it('buckets by the groups tags are in now, not when their chips were clicked', async () => {
    // p1 moves out of Priority: p0 and p1 stop ORing and start ANDing.
    const moved: TagState = {
      ...state,
      tags: state.tags.map((t) => (t.id === 'p1' ? tag('p1', null, 2, { name: 'P1' }) : t)),
    };
    expect(await kept(['tag:p0', 'tag:p1'])).toEqual(['a', 'b', 'c']);
    expect(await kept(['tag:p0', 'tag:p1'], { ctx: tagContext(moved) })).toEqual([]);
  });

  it('ignores keys naming tags the catalog no longer has', async () => {
    const { filterProjects } = await importHelpers();
    // Alone, they leave every project, as the same array.
    expect(filterProjects(projects, new Set(['tag:gone']), new Map(), never, never, tags)).toBe(
      projects
    );
    // With others, it's as if they weren't there.
    expect(await kept(['tag:gone', 'tag:q4'])).toEqual(['a', 'c']);
    expect(await kept(['tag:gone', 'unread'], { isUnread: (id) => id === 'e' })).toEqual(['e']);
  });

  it('ignores every tag key without a catalog', async () => {
    const { filterProjects } = await importHelpers();
    expect(filterProjects(projects, new Set(['tag:q4']), new Map(), never, never)).toBe(projects);
  });
});

describe('tagFilterClauses', () => {
  it('puts a group’s tags in one clause and each ungrouped tag in its own', async () => {
    const { tagFilterClauses } = await importHelpers();
    const keys = new Set(['tag:q4', 'tag:doing', 'unread', 'tag:p0', 'tag:done', 'tag:infra']);
    expect(tagFilterClauses(keys, tagContext(tagState()))).toEqual([
      ['q4'],
      ['doing', 'done'],
      ['p0'],
      ['infra'],
    ]);
  });
});

describe('effectiveFilterKeys', () => {
  const known = new Set(['q4', 'p0']);

  it('is the same set when every tag key is known', async () => {
    const { effectiveFilterKeys } = await importHelpers();
    const keys = new Set(['unread', 'repo:org/alpha:', 'tag:q4']);
    expect(effectiveFilterKeys(keys, known)).toBe(keys);
  });

  it('drops the keys of tags the catalog no longer has, keeping the rest', async () => {
    const { effectiveFilterKeys } = await importHelpers();
    const keys = new Set(['unread', 'tag:gone', 'repo:org/alpha:', 'tag:q4']);
    expect(effectiveFilterKeys(keys, known)).toEqual(
      new Set(['unread', 'repo:org/alpha:', 'tag:q4'])
    );
  });

  it('is empty, so Clear hides, when only stale tag keys remain', async () => {
    const { effectiveFilterKeys } = await importHelpers();
    expect(effectiveFilterKeys(new Set(['tag:gone', 'tag:also-gone']), known).size).toBe(0);
  });
});

describe('computeTagFilters / sectionTagFilters', () => {
  it('lists the tags live projects carry, in catalog order, with counts', async () => {
    const { computeTagFilters } = await importHelpers();
    const catalog = indexTagCatalog(tagState());
    const usage = new Map([
      ['q4', 2],
      ['done', 1],
      ['p0', 3],
    ]);
    expect(
      computeTagFilters(catalog, usage, new Set()).map((f) => [f.tag.id, f.group?.id, f.count])
    ).toEqual([
      ['done', 'status', 1],
      ['p0', 'priority', 3],
      ['q4', undefined, 2],
    ]);
  });

  it('keeps an active tag nobody carries any more, so its chip can turn it off', async () => {
    const { computeTagFilters } = await importHelpers();
    const catalog = indexTagCatalog(tagState());
    const filters = computeTagFilters(catalog, new Map([['q4', 1]]), new Set(['tag:infra']));
    expect(filters.map((f) => [f.tag.id, f.count])).toEqual([
      ['q4', 1],
      ['infra', 0],
    ]);
  });

  it('heads each group’s chips with its name and the ungrouped ones with “Tags”', async () => {
    const { computeTagFilters, sectionTagFilters } = await importHelpers();
    const catalog = indexTagCatalog(tagState());
    const usage = new Map([
      ['idea', 1],
      ['doing', 1],
      ['q4', 1],
      ['infra', 1],
    ]);
    expect(
      sectionTagFilters(computeTagFilters(catalog, usage, new Set())).map((section) => [
        section.key,
        section.label,
        section.filters.map((f) => f.tag.id),
      ])
    ).toEqual([
      ['group:status', 'Status', ['idea', 'doing']],
      ['ungrouped', 'Tags', ['q4', 'infra']],
    ]);
  });
});

describe('projectFiltersStore.toggleFilter', () => {
  async function importStore() {
    const { projectFiltersStore } = await importHelpers();
    return projectFiltersStore;
  }

  it('selects exclusively with no intent', async () => {
    const store = await importStore();
    store.toggleFilter('unread');
    store.toggleFilter('running');
    expect(store.activeFilters).toEqual(new Set(['running']));
  });

  it('reads additive intent from a shift-click event (chip bar)', async () => {
    const store = await importStore();
    store.toggleFilter('unread', { shiftKey: false } as MouseEvent);
    store.toggleFilter('running', { shiftKey: true } as MouseEvent);
    expect(store.activeFilters).toEqual(new Set(['unread', 'running']));
    store.toggleFilter({ repo: 'org/alpha', subpath: '' }, { shiftKey: false } as MouseEvent);
    expect(store.activeFilters).toEqual(new Set(['repo:org/alpha:']));
  });

  it('toggles tag filters by key', async () => {
    tagsMock.tagsById = new Map([['q4', {}]]);
    const store = await importStore();
    store.toggleFilter({ tagId: 'q4' });
    expect(store.activeFilters).toEqual(new Set(['tag:q4']));
    expect(store.isFilterActive({ tagId: 'q4' })).toBe(true);
    store.toggleFilter({ tagId: 'q4' });
    expect(store.activeFilters).toEqual(new Set());
  });

  it('drops a deleted tag’s key on the next toggle', async () => {
    tagsMock.tagsById = new Map([
      ['q4', {}],
      ['infra', {}],
    ]);
    const store = await importStore();
    store.toggleFilter({ tagId: 'q4' }, { additive: true });
    store.toggleFilter({ tagId: 'infra' }, { additive: true });
    tagsMock.tagsById = new Map([['infra', {}]]);
    // Only infra shows as active, so a plain click on it clears it.
    store.toggleFilter({ tagId: 'infra' });
    expect(store.activeFilters).toEqual(new Set());
  });

  it('takes an explicit { additive } option (filter menu)', async () => {
    const store = await importStore();
    store.toggleFilter('unread', { additive: true });
    store.toggleFilter('running', { additive: true });
    expect(store.activeFilters).toEqual(new Set(['unread', 'running']));
    store.toggleFilter('unread', { additive: true });
    expect(store.activeFilters).toEqual(new Set(['running']));
    store.toggleFilter('unread', { additive: false });
    expect(store.activeFilters).toEqual(new Set(['unread']));
  });
});
