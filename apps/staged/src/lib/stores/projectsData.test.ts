import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  Branch,
  PrStatusChangedEvent,
  Project,
  ProjectRepo,
  RepoHomeItem,
  SessionStatusPayload,
} from '../types';
import type { SwrResult } from '../cache';

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

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: 'r1',
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: '/wt/b1',
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

function homeRepo(overrides: Partial<RepoHomeItem> = {}): RepoHomeItem {
  return {
    githubRepo: 'org/alpha',
    subpath: '',
    shortName: 'alpha',
    hue: 120,
    createdAt: 0,
    pinned: false,
    pinSortOrder: null,
    defaultBranch: 'main',
    hasLocalClone: true,
    ...overrides,
  };
}

function prEvent(overrides: Partial<PrStatusChangedEvent> = {}): PrStatusChangedEvent {
  return {
    branchId: 'b1',
    prState: 'OPEN',
    prChecksStatus: 'PENDING',
    prReviewDecision: null,
    prMergeable: true,
    prDraft: false,
    prHeadSha: 'abc123',
    prFetchedAt: 1,
    failedChecks: [],
    ...overrides,
  };
}

function swr<T>(data: T, revalidating: Promise<T> | null = null): SwrResult<T> {
  return { data, revalidating };
}

// ── Mock plumbing ──

type EventCallback = (payload: unknown) => void;

let listProjects: ReturnType<typeof vi.fn>;
let listBranchesForProject: ReturnType<typeof vi.fn>;
let listProjectRepos: ReturnType<typeof vi.fn>;
let listReposForHome: ReturnType<typeof vi.fn>;
let invalidateProjectBranchTimelines: ReturnType<typeof vi.fn>;
let ensureForRepos: ReturnType<typeof vi.fn>;
let badgeLoadAll: ReturnType<typeof vi.fn>;
let eventListeners: Map<string, EventCallback[]>;
let windowTarget: EventTarget;

function emit(event: string, payload: unknown): void {
  for (const callback of eventListeners.get(event) ?? []) {
    callback(payload);
  }
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/**
 * Freeze the idle drip, so a test can hold a project in the known-but-
 * un-hydrated state the hydration guard turns on. scheduleDeferredTask prefers
 * requestIdleCallback over its setTimeout fallback, so a stub that never
 * invokes its callback means the drip is scheduled and never runs.
 */
function freezeBackgroundHydration(): void {
  vi.stubGlobal('requestIdleCallback', () => 1);
  vi.stubGlobal('cancelIdleCallback', () => {});
}

async function importStore() {
  const { projectsDataStore } = await import('./projectsData.svelte');
  return projectsDataStore;
}

beforeEach(() => {
  vi.resetModules();
  // The store's runes compile away in the app build; under vitest they stay
  // plain global calls, so stub $state as identity (agent.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  windowTarget = new EventTarget();
  vi.stubGlobal('window', windowTarget);

  eventListeners = new Map();
  listProjects = vi.fn().mockResolvedValue(swr([project()]));
  listBranchesForProject = vi.fn().mockResolvedValue(swr([branch()]));
  listProjectRepos = vi.fn().mockResolvedValue(swr([projectRepo()]));
  listReposForHome = vi.fn().mockResolvedValue([]);
  invalidateProjectBranchTimelines = vi.fn();
  ensureForRepos = vi.fn().mockResolvedValue(undefined);
  badgeLoadAll = vi.fn().mockResolvedValue(undefined);

  vi.doMock('../commands', () => ({
    listProjects,
    listBranchesForProject,
    listProjectRepos,
    listReposForHome,
    invalidateProjectBranchTimelines,
  }));
  vi.doMock('../transport', () => ({
    listenToEvent: (event: string, callback: EventCallback) => {
      const callbacks = eventListeners.get(event) ?? [];
      callbacks.push(callback);
      eventListeners.set(event, callbacks);
      return () => {
        const remaining = (eventListeners.get(event) ?? []).filter((cb) => cb !== callback);
        eventListeners.set(event, remaining);
      };
    },
  }));
  vi.doMock('./repoBadges.svelte', () => ({
    repoBadgeStore: {
      loadAll: badgeLoadAll,
      ensureForRepos,
    },
  }));
});

afterEach(() => {
  vi.doUnmock('../commands');
  vi.doUnmock('../transport');
  vi.doUnmock('./repoBadges.svelte');
  vi.unstubAllGlobals();
});

// ── Tests ──

describe('mergeBranchesPreservingWorktree', () => {
  async function importMerge() {
    const { mergeBranchesPreservingWorktree } = await import('./projectsData.svelte');
    return mergeBranchesPreservingWorktree;
  }

  it('preserves an existing worktreePath when the incoming branch has none', async () => {
    const merge = await importMerge();
    const merged = merge([branch({ worktreePath: '/wt/b1' })], [branch({ worktreePath: null })]);
    expect(merged).toHaveLength(1);
    expect(merged[0].worktreePath).toBe('/wt/b1');
  });

  it('takes the incoming worktreePath when it is populated', async () => {
    const merge = await importMerge();
    const merged = merge(
      [branch({ worktreePath: '/wt/old' })],
      [branch({ worktreePath: '/wt/new' })]
    );
    expect(merged[0].worktreePath).toBe('/wt/new');
  });

  it('adopts the incoming list shape: new branches added, missing ones dropped', async () => {
    const merge = await importMerge();
    const merged = merge(
      [branch({ id: 'gone' })],
      [branch({ id: 'b1' }), branch({ id: 'b2', worktreePath: null })]
    );
    expect(merged.map((b) => b.id)).toEqual(['b1', 'b2']);
  });
});

describe('ensureLoaded', () => {
  it('fetches the project list on first call, then hydrates on demand', async () => {
    const store = await importStore();
    expect(store.loaded).toBe(false);

    await store.ensureLoaded();

    expect(listProjects).toHaveBeenCalledTimes(1);
    expect(store.projects).toEqual([project()]);
    expect(store.branchesByProject.get('p1')).toEqual([]);
    expect(store.loaded).toBe(true);
    expect(store.loading).toBe(false);
    expect(store.error).toBeNull();

    await store.ensureProjectsHydrated();

    expect(listBranchesForProject).toHaveBeenCalledWith('p1');
    expect(listProjectRepos).toHaveBeenCalledWith('p1');
    expect(store.branchesByProject.get('p1')).toEqual([branch()]);
    expect(store.reposByProject.get('p1')).toEqual([projectRepo()]);
    expect(store.repoCountsByProject.get('p1')).toBe(1);
    expect(ensureForRepos).toHaveBeenCalled();
  });

  it('resolves while per-project hydration is still pending', async () => {
    // The regression this two-level readiness exists for: a cold start must
    // not sit behind every project's branches.
    listBranchesForProject.mockReturnValue(new Promise(() => {}));
    const store = await importStore();

    await store.ensureLoaded();

    expect(store.loaded).toBe(true);
    expect(store.loading).toBe(false);
    expect(store.projects).toEqual([project()]);
    expect(store.isProjectHydrated('p1')).toBe(false);
  });

  it('dedupes concurrent first loads into a single fetch', async () => {
    const store = await importStore();

    await Promise.all([store.ensureLoaded(), store.ensureLoaded()]);

    expect(listProjects).toHaveBeenCalledTimes(1);
  });

  it('resolves instantly once loaded and revalidates in the background', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    // Second load hangs — ensureLoaded must not wait for it.
    let resolveReload!: (value: SwrResult<Project[]>) => void;
    listProjects.mockReturnValueOnce(
      new Promise<SwrResult<Project[]>>((resolve) => {
        resolveReload = resolve;
      })
    );

    await store.ensureLoaded();
    expect(store.projects).toEqual([project()]);
    expect(listProjects).toHaveBeenCalledTimes(2);

    resolveReload(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    await vi.waitFor(() => {
      expect(store.projects).toHaveLength(2);
    });
    // New project's branch entry is seeded so consumers can render it.
    expect(store.branchesByProject.has('p2')).toBe(true);
  });

  it('prunes branches, repos, and hydration state of projects removed by a revalidation', async () => {
    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    expect(store.branchesByProject.has('p2')).toBe(true);
    expect(store.reposByProject.has('p2')).toBe(true);
    expect(store.isProjectHydrated('p2')).toBe(true);

    listProjects.mockResolvedValue(swr([project()]));
    await store.ensureLoaded();

    await vi.waitFor(() => {
      expect(store.projects).toHaveLength(1);
    });
    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(store.reposByProject.has('p2')).toBe(false);
    expect(store.isProjectHydrated('p2')).toBe(false);
  });

  it('applies the SwrResult revalidating promise when it resolves', async () => {
    let resolveFresh!: (value: Project[]) => void;
    listProjects.mockResolvedValueOnce(
      swr(
        [project()],
        new Promise<Project[]>((resolve) => {
          resolveFresh = resolve;
        })
      )
    );
    const store = await importStore();

    await store.ensureLoaded();
    expect(store.projects).toEqual([project()]);

    resolveFresh([project(), project({ id: 'p2', name: 'Beta' })]);
    await vi.waitFor(() => {
      expect(store.projects).toHaveLength(2);
    });
  });

  it('surfaces load failures via error and retries on the next call', async () => {
    listProjects.mockRejectedValueOnce(new Error('boom'));
    const store = await importStore();

    await store.ensureLoaded();
    expect(store.error).toBe('boom');
    expect(store.loaded).toBe(false);
    expect(store.loading).toBe(false);

    await store.ensureLoaded();
    expect(store.error).toBeNull();
    expect(store.loaded).toBe(true);
    expect(store.projects).toEqual([project()]);
  });
});

describe('hydration readiness', () => {
  it('flips isProjectHydrated per project and allProjectsHydrated on the last one', async () => {
    const branchesByProject = new Map<string, (value: SwrResult<Branch[]>) => void>();
    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    listBranchesForProject.mockImplementation(
      (projectId: string) =>
        new Promise<SwrResult<Branch[]>>((resolve) => {
          branchesByProject.set(projectId, resolve);
        })
    );
    const store = await importStore();

    await store.ensureLoaded();
    const hydrating = store.ensureProjectsHydrated();
    await vi.waitFor(() => {
      expect(branchesByProject.size).toBe(2);
    });
    expect(store.isProjectHydrated('p1')).toBe(false);
    expect(store.allProjectsHydrated).toBe(false);

    branchesByProject.get('p1')!(swr([branch()]));
    await vi.waitFor(() => {
      expect(store.isProjectHydrated('p1')).toBe(true);
    });
    expect(store.allProjectsHydrated).toBe(false);

    branchesByProject.get('p2')!(swr([]));
    await hydrating;
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('marks a project settled even when its branches fail to load', async () => {
    listBranchesForProject.mockRejectedValue(new Error('boom'));
    const store = await importStore();

    await store.ensureLoaded();
    await store.ensureProjectsHydrated();

    // Settled, not successful — a view gated on hydration must never hang.
    expect(store.isProjectHydrated('p1')).toBe(true);
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('dedupes the idle drip and a foreground hydrate into one fetch', async () => {
    const store = await importStore();

    await store.ensureLoaded();
    await store.hydrateProject('p1');
    await tick();

    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(listProjectRepos).toHaveBeenCalledTimes(1);
  });

  it('ensureProjectHydrated fetches a project nobody has hydrated yet', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    expect(store.isProjectHydrated('p1')).toBe(false);

    await store.ensureProjectHydrated('p1');

    expect(store.isProjectHydrated('p1')).toBe(true);
    expect(store.branchesByProject.get('p1')).toEqual([branch()]);
    expect(store.reposByProject.get('p1')).toEqual([projectRepo()]);
    // The drip skips what the ensure call already fetched under this load.
    await tick();
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
  });

  it('ensureProjectHydrated no-ops for an already hydrated project', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    listBranchesForProject.mockClear();
    listProjectRepos.mockClear();

    await store.ensureProjectHydrated('p1');
    await tick();

    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
  });

  it('ensureProjectHydrated joins an in-flight drip instead of refetching', async () => {
    let resolveBranches!: (value: SwrResult<Branch[]>) => void;
    listBranchesForProject.mockReturnValue(
      new Promise<SwrResult<Branch[]>>((resolve) => {
        resolveBranches = resolve;
      })
    );
    const store = await importStore();
    await store.ensureLoaded();
    // Let the idle drip pick p1 up first.
    await vi.waitFor(() => {
      expect(listBranchesForProject).toHaveBeenCalledWith('p1');
    });

    const ensuring = store.ensureProjectHydrated('p1');
    resolveBranches(swr([branch()]));
    await ensuring;

    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(store.isProjectHydrated('p1')).toBe(true);
  });

  it('ensureProjectsHydrated awaits in-flight work instead of refetching', async () => {
    let resolveBranches!: (value: SwrResult<Branch[]>) => void;
    listBranchesForProject.mockReturnValueOnce(
      new Promise<SwrResult<Branch[]>>((resolve) => {
        resolveBranches = resolve;
      })
    );
    const store = await importStore();
    await store.ensureLoaded();

    const foreground = store.hydrateProject('p1');
    const sweep = store.ensureProjectsHydrated();
    resolveBranches(swr([branch()]));
    await Promise.all([foreground, sweep]);

    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(store.isProjectHydrated('p1')).toBe(true);
  });
});

describe('hydrateProject', () => {
  it('merges refetched branches, preserving worktreePath over a stale null', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    expect(store.branchesByProject.get('p1')![0].worktreePath).toBe('/wt/b1');

    listBranchesForProject.mockResolvedValue(
      swr([branch({ worktreePath: null, prState: 'OPEN' })])
    );
    await store.hydrateProject('p1');

    const hydrated = store.branchesByProject.get('p1')![0];
    expect(hydrated.prState).toBe('OPEN');
    expect(hydrated.worktreePath).toBe('/wt/b1');
  });

  it('applies the branches SwrResult revalidating promise', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    let resolveFresh!: (value: Branch[]) => void;
    listBranchesForProject.mockResolvedValueOnce(
      swr(
        [branch()],
        new Promise<Branch[]>((resolve) => {
          resolveFresh = resolve;
        })
      )
    );
    await store.hydrateProject('p1');

    resolveFresh([branch({ prState: 'MERGED' })]);
    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')![0].prState).toBe('MERGED');
    });
  });

  it('discards a stale hydration superseded by a refresh (generation guard)', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();

    let resolveStale!: (value: SwrResult<Branch[]>) => void;
    listBranchesForProject.mockReturnValueOnce(
      new Promise<SwrResult<Branch[]>>((resolve) => {
        resolveStale = resolve;
      })
    );
    const staleHydration = store.hydrateProject('p1');

    // refresh() bumps the generation and lands fresh data.
    listBranchesForProject.mockResolvedValue(swr([branch({ branchName: 'fresh' })]));
    await store.refresh();
    expect(store.branchesByProject.get('p1')![0].branchName).toBe('fresh');

    // The pre-refresh response resolving late must not clobber it.
    resolveStale(swr([branch({ branchName: 'stale' })]));
    await staleHydration;
    expect(store.branchesByProject.get('p1')![0].branchName).toBe('fresh');
  });

  it('defers background-priority hydration off the critical path', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    listBranchesForProject.mockClear();

    await store.hydrateProject('p1', { priority: 'background' });
    expect(listBranchesForProject).not.toHaveBeenCalled();

    await vi.waitFor(() => {
      expect(listBranchesForProject).toHaveBeenCalledWith('p1');
    });
  });
});

describe('refreshProject', () => {
  it('refetches the project list and one project, invalidating its branch timelines', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    listProjects.mockResolvedValue(swr([project({ name: 'Renamed' })]));
    listBranchesForProject.mockResolvedValue(
      swr([branch(), branch({ id: 'b2', worktreePath: null })])
    );
    await store.refreshProject('p1');

    expect(store.projects[0].name).toBe('Renamed');
    expect(store.branchesByProject.get('p1')).toHaveLength(2);
    expect(invalidateProjectBranchTimelines).toHaveBeenCalledWith(['b1', 'b2']);
  });
});

describe('projectCreated', () => {
  it('registers the project immediately and hydrates it in the background', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    const created = project({ id: 'p2', name: 'Beta', githubRepo: 'org/beta' });
    listBranchesForProject.mockResolvedValue(swr([branch({ id: 'b2', projectId: 'p2' })]));
    listProjectRepos.mockResolvedValue(swr([projectRepo({ id: 'r2', projectId: 'p2' })]));
    store.projectCreated(created);

    // Synchronous registration so the creation modal can close instantly, and
    // hydrated right away so selecting it doesn't blank the project view.
    expect(store.projects.map((p) => p.id)).toEqual(['p1', 'p2']);
    expect(store.branchesByProject.get('p2')).toEqual([]);
    expect(store.isProjectHydrated('p2')).toBe(true);

    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p2')).toHaveLength(1);
    });
    expect(store.reposByProject.get('p2')).toHaveLength(1);
  });

  it('does not duplicate a project the store already knows', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    store.projectCreated(project());

    expect(store.projects).toHaveLength(1);
  });
});

describe('setBranchesByProject', () => {
  it('replaces the branch map for view-driven updates', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    const renamed = branch({ branchName: 'renamed' });
    store.setBranchesByProject(new Map(store.branchesByProject).set('p1', [renamed]));

    expect(store.branchesByProject.get('p1')).toEqual([renamed]);
  });
});

describe('refresh', () => {
  it('reloads the project list and rehydrates what was already hydrated', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();

    listProjects.mockResolvedValue(swr([project({ name: 'Renamed' })]));
    listBranchesForProject.mockResolvedValue(swr([branch({ prState: 'MERGED' })]));
    await store.refresh();

    expect(store.projects[0].name).toBe('Renamed');
    expect(store.branchesByProject.get('p1')![0].prState).toBe('MERGED');
  });

  it('refetches home repos when they were previously loaded', async () => {
    listReposForHome.mockResolvedValue([homeRepo()]);
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureHomeReposLoaded();

    listReposForHome.mockResolvedValue([homeRepo(), homeRepo({ githubRepo: 'org/beta' })]);
    await store.refresh();

    await vi.waitFor(() => {
      expect(store.homeRepos).toHaveLength(2);
    });
  });
});

describe('home repos cache', () => {
  it('refreshHomeRepos forces a refetch', async () => {
    listReposForHome.mockResolvedValue([homeRepo()]);
    const store = await importStore();
    await store.ensureHomeReposLoaded();

    listReposForHome.mockResolvedValue([homeRepo({ hasLocalClone: false })]);
    await store.refreshHomeRepos();

    expect(store.homeRepos[0].hasLocalClone).toBe(false);
  });

  it('fetches once, dedupes concurrent callers, then serves from memory', async () => {
    let resolveRepos!: (value: RepoHomeItem[]) => void;
    listReposForHome.mockReturnValueOnce(
      new Promise<RepoHomeItem[]>((resolve) => {
        resolveRepos = resolve;
      })
    );
    const store = await importStore();
    expect(store.homeReposLoaded).toBe(false);

    const first = store.ensureHomeReposLoaded();
    const second = store.ensureHomeReposLoaded();
    resolveRepos([homeRepo()]);
    await Promise.all([first, second]);

    expect(listReposForHome).toHaveBeenCalledTimes(1);
    expect(store.homeReposLoaded).toBe(true);
    expect(store.homeRepos).toEqual([homeRepo()]);
    expect(store.homeReposLoading).toBe(false);

    // Later calls resolve instantly and revalidate in the background.
    listReposForHome.mockResolvedValue([homeRepo(), homeRepo({ githubRepo: 'org/beta' })]);
    await store.ensureHomeReposLoaded();
    await vi.waitFor(() => {
      expect(store.homeRepos).toHaveLength(2);
    });
  });
});

describe('event listeners', () => {
  it('registers listeners only once across repeated startListeners calls', async () => {
    const store = await importStore();
    store.startListeners();
    store.startListeners();

    expect(eventListeners.get('pr-status-changed')).toHaveLength(1);
    expect(eventListeners.get('session-status-changed')).toHaveLength(1);
    expect(eventListeners.get('project-setup-progress')).toHaveLength(1);
    expect(eventListeners.get('project-changed')).toHaveLength(1);
    expect(eventListeners.get('branch-changed')).toHaveLength(1);
    expect(eventListeners.get('repos-changed')).toHaveLength(1);
  });

  it('coalesces a pr-status-changed burst into one flush, last event winning', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    store.startListeners();

    emit('pr-status-changed', prEvent({ prState: 'OPEN', prChecksStatus: 'PENDING' }));
    emit('pr-status-changed', prEvent({ prState: 'MERGED', prChecksStatus: 'SUCCESS' }));

    // Buffered — nothing applied until the frame flush.
    expect(store.branchesByProject.get('p1')![0].prState).toBeNull();

    await tick();
    const updated = store.branchesByProject.get('p1')![0];
    expect(updated.prState).toBe('MERGED');
    expect(updated.prChecksStatus).toBe('SUCCESS');
    // Untouched fields survive the update.
    expect(updated.worktreePath).toBe('/wt/b1');
  });

  it('refetches a project’s branches when a commit session completes', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    store.startListeners();
    listBranchesForProject.mockClear();
    listBranchesForProject.mockResolvedValue(swr([branch({ prState: 'OPEN' })]));

    emit('session-status-changed', {
      sessionId: 's1',
      status: 'completed',
      sessionType: 'commit',
      projectId: 'p1',
    } satisfies SessionStatusPayload);

    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')![0].prState).toBe('OPEN');
    });
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
  });

  it('ignores non-commit sessions and un-hydrated projects', async () => {
    freezeBackgroundHydration();
    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectHydrated('p1');
    store.startListeners();
    listBranchesForProject.mockClear();

    emit('session-status-changed', {
      sessionId: 's1',
      status: 'completed',
      sessionType: 'plan',
      projectId: 'p1',
    } satisfies SessionStatusPayload);
    emit('session-status-changed', {
      sessionId: 's2',
      status: 'completed',
      sessionType: 'commit',
      projectId: 'unknown',
    } satisfies SessionStatusPayload);
    // p2 is listed but was never hydrated, so no sprout icon is painted for it
    // — there is nothing for the refetch to flip.
    emit('session-status-changed', {
      sessionId: 's3',
      status: 'completed',
      sessionType: 'commit',
      projectId: 'p2',
    } satisfies SessionStatusPayload);

    await tick();
    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('refreshes the project list and one project on setup progress', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    store.startListeners();

    listProjects.mockResolvedValue(swr([project({ name: 'Renamed' })]));
    listBranchesForProject.mockResolvedValue(
      swr([branch(), branch({ id: 'b2', worktreePath: null })])
    );
    emit('project-setup-progress', 'p1');

    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')).toHaveLength(2);
    });
    expect(store.projects[0].name).toBe('Renamed');
    expect(invalidateProjectBranchTimelines).toHaveBeenCalledWith(['b1', 'b2']);
  });

  it('reloads everything on cache-stale', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    store.startListeners();
    expect(listProjects).toHaveBeenCalledTimes(1);

    windowTarget.dispatchEvent(new Event('cache-stale'));

    await vi.waitFor(() => {
      expect(listProjects).toHaveBeenCalledTimes(2);
    });
  });

  it('reloads the project list on project-changed', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    store.startListeners();

    listProjects.mockResolvedValue(swr([project({ name: 'Renamed' })]));
    emit('project-changed', { projectId: 'p1' });

    await vi.waitFor(() => {
      expect(store.projects[0].name).toBe('Renamed');
    });
  });

  it('registers a project created in another window on project-changed', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    store.startListeners();

    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    emit('project-changed', { projectId: 'p2' });

    await vi.waitFor(() => {
      expect(store.projects.map((p) => p.id)).toEqual(['p1', 'p2']);
    });
  });

  it('refetches the resolved project’s branches on branch-changed, coalescing a burst', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    store.startListeners();
    listBranchesForProject.mockClear();
    listBranchesForProject.mockResolvedValue(swr([branch({ branchName: 'renamed' })]));

    emit('branch-changed', { branchId: 'b1', projectId: 'p1' });
    emit('branch-changed', { branchId: 'b1', projectId: 'p1' });

    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')![0].branchName).toBe('renamed');
    });
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
  });

  it('falls back to projects holding the branch when branch-changed lacks a project', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    store.startListeners();
    listBranchesForProject.mockClear();
    listBranchesForProject.mockResolvedValue(swr([]));

    emit('branch-changed', { branchId: 'b1', projectId: null });

    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')).toHaveLength(0);
    });
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).toHaveBeenCalledWith('p1');
  });

  it('refetches every hydrated project’s branches when branch-changed names none', async () => {
    freezeBackgroundHydration();
    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    listBranchesForProject.mockImplementation((projectId: string) =>
      Promise.resolve(swr([branch({ id: `${projectId}-b1`, projectId })]))
    );
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectHydrated('p1');
    store.startListeners();
    listBranchesForProject.mockClear();

    // The lag flush: the feed dropped changes it can no longer name, so it
    // expands to every project the store paints — p2, never hydrated, is not
    // one of them even though applyProjectList seeded it a branch entry.
    emit('branch-changed', { branchId: null, projectId: null });
    await tick();

    expect(listBranchesForProject.mock.calls.map(([projectId]) => projectId)).toEqual(['p1']);
  });

  it('skips branch-changed refetches for a known but un-hydrated project', async () => {
    freezeBackgroundHydration();
    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectHydrated('p1');
    store.startListeners();
    listBranchesForProject.mockClear();

    // The state _branchesByProject.has() cannot distinguish: p2 is in the
    // fetched list, so it holds a seeded empty entry, but nobody ever fetched
    // it and no view painted it.
    expect(store.branchesByProject.has('p2')).toBe(true);
    expect(store.isProjectHydrated('p2')).toBe(false);

    emit('branch-changed', { branchId: 'p2-b1', projectId: 'p2' });
    await tick();

    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('chains a branch-changed refetch onto a first hydration still in flight', async () => {
    freezeBackgroundHydration();
    let resolveBranches!: (value: SwrResult<Branch[]>) => void;
    listBranchesForProject.mockReturnValueOnce(
      new Promise<SwrResult<Branch[]>>((resolve) => {
        resolveBranches = resolve;
      })
    );
    const store = await importStore();
    await store.ensureLoaded();
    store.startListeners();

    // The first hydration is mid-flight, so its read may predate the mutation
    // the event announces — skipping outright would paint that stale read and
    // leave it until the project's next event.
    const hydrating = store.hydrateProject('p1');
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);

    emit('branch-changed', { branchId: 'b1', projectId: 'p1' });
    await tick();
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);

    listBranchesForProject.mockResolvedValue(swr([branch({ branchName: 'renamed' })]));
    resolveBranches(swr([branch({ branchName: 'stale' })]));
    await hydrating;

    // Exactly one follow-up, and it applies after the hydration it chained onto.
    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')![0].branchName).toBe('renamed');
    });
    expect(listBranchesForProject).toHaveBeenCalledTimes(2);
  });

  it('skips branch-changed refetches for unknown or deleting projects', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    store.startListeners();
    listBranchesForProject.mockClear();

    store.projectDeleteStarted('p1', 'Alpha');
    emit('branch-changed', { branchId: 'b1', projectId: 'p1' });
    emit('branch-changed', { branchId: 'b-elsewhere', projectId: 'unknown' });
    await tick();

    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('reloads badges and home repos on repos-changed', async () => {
    listReposForHome.mockResolvedValue([homeRepo()]);
    const store = await importStore();
    store.startListeners();
    await store.ensureHomeReposLoaded();
    badgeLoadAll.mockClear();

    listReposForHome.mockResolvedValue([homeRepo({ pinned: true })]);
    emit('repos-changed', { githubRepo: 'org/alpha' });

    await vi.waitFor(() => {
      expect(store.homeRepos[0].pinned).toBe(true);
    });
    expect(badgeLoadAll).toHaveBeenCalledTimes(1);
  });

  it('does not fetch home repos on repos-changed before anyone loaded them', async () => {
    const store = await importStore();
    store.startListeners();

    emit('repos-changed', { githubRepo: 'org/alpha' });
    await tick();

    expect(listReposForHome).not.toHaveBeenCalled();
  });

  it('stopListeners unregisters everything', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    store.startListeners();
    store.stopListeners();

    expect(eventListeners.get('pr-status-changed')).toHaveLength(0);
    windowTarget.dispatchEvent(new Event('cache-stale'));
    await tick();
    expect(listProjects).toHaveBeenCalledTimes(1);
  });
});

describe('project-delete lifecycle', () => {
  it('tracks the deleting project until the post-delete refetch prunes it', async () => {
    const store = await importStore();
    store.startListeners();
    await store.ensureLoaded();

    store.projectDeleteStarted('p1', 'Alpha');
    expect(store.isProjectDeleting('p1')).toBe(true);
    expect(store.deletingProjectNames.get('p1')).toBe('Alpha');

    // The backend delete publishes project-changed; the refetch removes the
    // project and its "Deleting…" marker in one apply.
    listProjects.mockResolvedValue(swr([]));
    emit('project-changed', { projectId: 'p1' });

    await vi.waitFor(() => {
      expect(store.projects).toHaveLength(0);
    });
    expect(store.isProjectDeleting('p1')).toBe(false);
    expect(store.branchesByProject.has('p1')).toBe(false);
    expect(store.reposByProject.has('p1')).toBe(false);
  });

  it('keeps the project and repairs its branch list when a delete fails', async () => {
    listBranchesForProject.mockResolvedValue(swr([branch(), branch({ id: 'b2' })]));
    const store = await importStore();
    store.startListeners();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    listBranchesForProject.mockClear();

    store.projectDeleteStarted('p1', 'Alpha');

    // The cascade's per-branch events are dropped while the delete is in
    // flight — refetching a doomed list N times would be pure churn.
    emit('branch-changed', { branchId: 'b2', projectId: 'p1' });
    await tick();
    expect(listBranchesForProject).not.toHaveBeenCalled();

    // The delete failed after the cascade already deleted b2's row, so the
    // dropped events are the ones that would have pruned it: clearing the
    // marker has to refetch, or the card lists a branch that no longer exists.
    listBranchesForProject.mockResolvedValue(swr([branch()]));
    store.projectDeleteFailed('p1');

    expect(store.isProjectDeleting('p1')).toBe(false);
    expect(store.projects).toHaveLength(1);
    expect(listBranchesForProject).toHaveBeenCalledWith('p1');
    await vi.waitFor(() => {
      expect(store.branchesByProject.get('p1')!.map((b) => b.id)).toEqual(['b1']);
    });
  });

  it('does not fetch branches for an un-hydrated project when a delete fails', async () => {
    const store = await importStore();
    await store.ensureLoaded();
    await store.ensureProjectsHydrated();
    listBranchesForProject.mockClear();

    // No branch list was ever painted for this project, so there is nothing to
    // repair — and fetching would insert a map entry the store never loaded.
    store.projectDeleteFailed('p-unknown');
    await tick();

    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(store.branchesByProject.has('p-unknown')).toBe(false);
  });

  it('does not fetch branches when a delete fails for a known but un-hydrated project', async () => {
    freezeBackgroundHydration();
    const store = await importStore();
    await store.ensureLoaded();
    listBranchesForProject.mockClear();

    // p1 is listed, so applyProjectList seeded it a branch entry — but nothing
    // fetched or painted it, and a fetch here would half-hydrate it: branches
    // without repos, unmarked, so the idle drip would refetch it anyway.
    expect(store.branchesByProject.has('p1')).toBe(true);
    store.projectDeleteFailed('p1');
    await tick();

    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('keeps the deleting marker through an apply that still contains the project', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    store.projectDeleteStarted('p1', 'Alpha');
    // A reload that read the list before the backend delete committed: the
    // project stays, but so does its marker — the card must not flash back
    // to a live project mid-delete.
    await store.ensureLoaded(); // kicks a background revalidation
    await tick();

    expect(store.projects).toHaveLength(1);
    expect(store.isProjectDeleting('p1')).toBe(true);
  });

  it('discards an SWR revalidation that resolves after the post-delete refetch', async () => {
    const beta = project({ id: 'p2', name: 'Beta' });
    let resolveFresh!: (value: Project[]) => void;
    listProjects.mockResolvedValueOnce(
      swr(
        [project(), beta],
        new Promise<Project[]>((resolve) => {
          resolveFresh = resolve;
        })
      )
    );
    const store = await importStore();
    store.startListeners();
    await store.ensureLoaded();

    store.projectDeleteStarted('p2', 'Beta');
    listProjects.mockResolvedValue(swr([project()]));
    emit('project-changed', { projectId: 'p2' });
    await vi.waitFor(() => {
      expect(store.projects.map((p) => p.id)).toEqual(['p1']);
    });

    // Fetched before the backend delete — applying it would resurrect p2,
    // but the post-delete reload's generation bump discards it.
    resolveFresh([project(), beta]);
    await tick();

    expect(store.projects.map((p) => p.id)).toEqual(['p1']);
    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(store.isProjectDeleting('p2')).toBe(false);
  });

  it("discards refreshProject's list replacement racing the delete", async () => {
    const beta = project({ id: 'p2', name: 'Beta' });
    listProjects.mockResolvedValue(swr([project(), beta]));
    const store = await importStore();
    store.startListeners();
    await store.ensureLoaded();

    let resolveList!: (value: SwrResult<Project[]>) => void;
    listProjects.mockReturnValueOnce(
      new Promise<SwrResult<Project[]>>((resolve) => {
        resolveList = resolve;
      })
    );
    const refreshing = store.refreshProject('p1');

    store.projectDeleteStarted('p2', 'Beta');
    listProjects.mockResolvedValue(swr([project()]));
    emit('project-changed', { projectId: 'p2' });
    await vi.waitFor(() => {
      expect(store.projects.map((p) => p.id)).toEqual(['p1']);
    });

    resolveList(swr([project(), beta]));
    await refreshing;

    expect(store.projects.map((p) => p.id)).toEqual(['p1']);
  });
});

describe('repoCountsByProject', () => {
  it('falls back to 1 for un-hydrated single-repo projects, 0 otherwise', async () => {
    let resolveRepos!: (value: SwrResult<ProjectRepo[]>) => void;
    listProjects.mockResolvedValue(
      swr([project(), project({ id: 'p2', name: 'Beta', githubRepo: null })])
    );
    listProjectRepos.mockReturnValue(
      new Promise<SwrResult<ProjectRepo[]>>((resolve) => {
        resolveRepos = resolve;
      })
    );
    const store = await importStore();
    await store.ensureLoaded();
    const hydrating = store.ensureProjectsHydrated();

    expect(store.projects).toHaveLength(2);
    expect(store.repoCountsByProject.get('p1')).toBe(1);
    expect(store.repoCountsByProject.get('p2')).toBe(0);

    resolveRepos(
      swr([projectRepo(), projectRepo({ id: 'r2', githubRepo: 'org/other', subpath: 'pkg' })])
    );
    await hydrating;
    expect(store.repoCountsByProject.get('p1')).toBe(2);
  });
});
