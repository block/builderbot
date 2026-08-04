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
      loadAll: vi.fn().mockResolvedValue(undefined),
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
  it('performs the full fetch on first call: projects, branches, and repos', async () => {
    const store = await importStore();
    expect(store.loaded).toBe(false);

    await store.ensureLoaded();

    expect(listProjects).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).toHaveBeenCalledWith('p1');
    expect(listProjectRepos).toHaveBeenCalledWith('p1');
    expect(store.projects).toEqual([project()]);
    expect(store.branchesByProject.get('p1')).toEqual([branch()]);
    expect(store.reposByProject.get('p1')).toEqual([projectRepo()]);
    expect(store.repoCountsByProject.get('p1')).toBe(1);
    expect(store.loaded).toBe(true);
    expect(store.loading).toBe(false);
    expect(store.error).toBeNull();
    expect(ensureForRepos).toHaveBeenCalled();
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

  it('prunes branches and repos of projects removed by a revalidation', async () => {
    listProjects.mockResolvedValue(swr([project(), project({ id: 'p2', name: 'Beta' })]));
    const store = await importStore();
    await store.ensureLoaded();
    expect(store.branchesByProject.has('p2')).toBe(true);
    expect(store.reposByProject.has('p2')).toBe(true);

    listProjects.mockResolvedValue(swr([project()]));
    await store.ensureLoaded();

    await vi.waitFor(() => {
      expect(store.projects).toHaveLength(1);
    });
    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(store.reposByProject.has('p2')).toBe(false);
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

describe('hydrateProject', () => {
  it('merges refetched branches, preserving worktreePath over a stale null', async () => {
    const store = await importStore();
    await store.ensureLoaded();
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
    listBranchesForProject.mockClear();

    await store.hydrateProject('p1', { priority: 'background' });
    expect(listBranchesForProject).not.toHaveBeenCalled();

    await vi.waitFor(() => {
      expect(listBranchesForProject).toHaveBeenCalledWith('p1');
    });
  });
});

describe('refresh', () => {
  it('reloads the project list and rehydrates eagerly', async () => {
    const store = await importStore();
    await store.ensureLoaded();

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
  });

  it('coalesces a pr-status-changed burst into one flush, last event winning', async () => {
    const store = await importStore();
    await store.ensureLoaded();
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

  it('ignores non-commit sessions and unknown projects', async () => {
    const store = await importStore();
    await store.ensureLoaded();
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

    await tick();
    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('refreshes the project list and one project on setup progress', async () => {
    const store = await importStore();
    await store.ensureLoaded();
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

  it('refetches home repos when pinned repos change', async () => {
    listReposForHome.mockResolvedValue([homeRepo()]);
    const store = await importStore();
    store.startListeners();
    await store.ensureHomeReposLoaded();

    listReposForHome.mockResolvedValue([homeRepo({ pinned: true })]);
    windowTarget.dispatchEvent(new Event('staged:pinned-repos-changed'));

    await vi.waitFor(() => {
      expect(store.homeRepos[0].pinned).toBe(true);
    });
  });

  it('does not fetch home repos on pin changes before anyone loaded them', async () => {
    const store = await importStore();
    store.startListeners();

    windowTarget.dispatchEvent(new Event('staged:pinned-repos-changed'));
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
  it('tracks deleting projects and prunes state when removal completes', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    store.projectDeleteStarted('p1', 'Alpha');
    expect(store.isProjectDeleting('p1')).toBe(true);
    expect(store.deletingProjectNames.get('p1')).toBe('Alpha');

    store.projectDeleteFinished('p1', { removed: true });
    expect(store.isProjectDeleting('p1')).toBe(false);
    expect(store.projects).toHaveLength(0);
    expect(store.branchesByProject.has('p1')).toBe(false);
    expect(store.reposByProject.has('p1')).toBe(false);
  });

  it('keeps the project when a delete fails', async () => {
    const store = await importStore();
    await store.ensureLoaded();

    store.projectDeleteStarted('p1', 'Alpha');
    store.projectDeleteFinished('p1');

    expect(store.isProjectDeleting('p1')).toBe(false);
    expect(store.projects).toHaveLength(1);
    expect(store.branchesByProject.has('p1')).toBe(true);
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
    const load = store.ensureLoaded();

    await vi.waitFor(() => {
      expect(store.projects).toHaveLength(2);
    });
    expect(store.repoCountsByProject.get('p1')).toBe(1);
    expect(store.repoCountsByProject.get('p2')).toBe(0);

    resolveRepos(
      swr([projectRepo(), projectRepo({ id: 'r2', githubRepo: 'org/other', subpath: 'pkg' })])
    );
    await load;
    expect(store.repoCountsByProject.get('p1')).toBe(2);
  });
});
