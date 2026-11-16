// The bulk hydration path of projectsDataStore: many projects hydrated with
// two all-projects reads and applied once. What it must keep from the
// one-project path is the subject here: the load generation check, each
// project's read ordering against a later read or a local write of that
// project, the worktree-preserving merge, and the web build's cached read
// followed by its revalidated one.
//
// In the "component" vitest project for the same reason as
// projectsData.component.test.ts: the store's runes have to compile for the
// client.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../types';
import type { SwrResult } from '../cache';

// ── Fixtures ──

/** Enough projects for the store to hydrate them in bulk. */
const IDS = ['p1', 'p2', 'p3', 'p4'];

function project(id: string): Project {
  return {
    id,
    name: id,
    githubRepo: `org/${id}`,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

/** A project's one branch. `branchName` marks which read a list came from. */
function branch(projectId: string, overrides: Partial<Branch> = {}): Branch {
  return {
    id: `${projectId}-b`,
    projectId,
    projectRepoId: `${projectId}-r`,
    branchName: 'bulk',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: `/wt/${projectId}`,
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
    deferredPush: null,
    deferredPushRequestedAt: null,
    ...overrides,
  };
}

function repo(projectId: string, overrides: Partial<ProjectRepo> = {}): ProjectRepo {
  return {
    id: `${projectId}-r`,
    projectId,
    githubRepo: `org/${projectId}`,
    branchName: 'bulk',
    subpath: null,
    isPrimary: true,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

/** An all-projects branch reply: one branch per project, named `name`. */
function branchesOf(name: string, ids: string[] = IDS): Record<string, Branch[]> {
  return Object.fromEntries(ids.map((id) => [id, [branch(id, { branchName: name })]]));
}

/** An all-projects repo reply: one repo per project, on branch `name`. */
function reposOf(name: string, ids: string[] = IDS): Record<string, ProjectRepo[]> {
  return Object.fromEntries(ids.map((id) => [id, [repo(id, { branchName: name })]]));
}

function swr<T>(data: T, revalidating: Promise<T> | null = null): SwrResult<T> {
  return { data, revalidating };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// ── Mock plumbing ──

type EventCallback = (payload: unknown) => void;

let listProjects: ReturnType<typeof vi.fn>;
let listBranchesForProject: ReturnType<typeof vi.fn>;
let listProjectRepos: ReturnType<typeof vi.fn>;
let listBranchesByProject: ReturnType<typeof vi.fn>;
let listProjectReposByProject: ReturnType<typeof vi.fn>;
let ensureForRepos: ReturnType<typeof vi.fn>;
let eventListeners: Map<string, EventCallback[]>;
let windowTarget: EventTarget;

function emit(event: string, payload: unknown): void {
  for (const callback of eventListeners.get(event) ?? []) callback(payload);
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** Let every reply already resolved, and what it kicked off, land. */
async function settle(): Promise<void> {
  for (let i = 0; i < 4; i++) await tick();
}

/**
 * Hold the idle queue, so background hydration is scheduled and runs only
 * when the test calls `run()`. scheduleDeferredTask prefers
 * requestIdleCallback over its setTimeout fallback.
 */
function holdIdleQueue(): { run: () => void } {
  const tasks = new Map<number, () => void>();
  let nextHandle = 1;
  vi.stubGlobal('requestIdleCallback', (callback: () => void) => {
    tasks.set(nextHandle, callback);
    return nextHandle++;
  });
  vi.stubGlobal('cancelIdleCallback', (handle: number) => {
    tasks.delete(handle);
  });
  return {
    run: () => {
      const due = [...tasks.values()];
      tasks.clear();
      for (const task of due) task();
    },
  };
}

async function importStore() {
  const { projectsDataStore } = await import('./projectsData.svelte');
  return projectsDataStore;
}

type Store = Awaited<ReturnType<typeof importStore>>;

/**
 * Record what `read()` returns each time its readers are woken: once now,
 * then once per change, the way a card's $derived or a template re-runs.
 * Imported after the store, so both share the module graph (and with it the
 * Svelte runtime) that beforeEach's resetModules started. `flush()` runs
 * pending effects and returns how many values were recorded since the last
 * call.
 */
async function watch<T>(read: () => T): Promise<{ flush: () => T[]; stop: () => void }> {
  const { readReactively } = await import('./testStubs/readReactively.svelte');
  const { flushSync } = await import('svelte');
  let seen: T[] = [];
  const stop = readReactively(() => {
    seen.push(read());
  });
  return {
    flush: () => {
      flushSync();
      const since = seen;
      seen = [];
      return since;
    },
    stop,
  };
}

/** A loaded store with the idle queue held, so nothing is hydrated yet. */
async function loadedStore(): Promise<{ store: Store; idle: { run: () => void } }> {
  const idle = holdIdleQueue();
  const store = await importStore();
  await store.ensureLoaded();
  return { store, idle };
}

/** The branch name each project shows: which read its list came from. */
function branchNames(store: Store, ids: string[] = IDS): Record<string, string | undefined> {
  return Object.fromEntries(
    ids.map((id) => [id, store.branchesByProject.get(id)?.[0]?.branchName])
  );
}

function repoBranchNames(store: Store, ids: string[] = IDS): Record<string, string | undefined> {
  return Object.fromEntries(ids.map((id) => [id, store.reposByProject.get(id)?.[0]?.branchName]));
}

beforeEach(() => {
  vi.resetModules();
  // As in projectsData.component.test.ts: the store's per-frame flushes fall
  // back to setTimeout, which the tests' ticks wait on.
  vi.stubGlobal('requestAnimationFrame', undefined);
  windowTarget = new EventTarget();
  vi.stubGlobal('window', windowTarget);

  eventListeners = new Map();
  listProjects = vi.fn().mockResolvedValue(swr(IDS.map(project)));
  // The one-project reads answer with lists named `single`, so a test can
  // tell which read a project's list came from.
  listBranchesForProject = vi.fn(async (projectId: string) =>
    swr([branch(projectId, { branchName: 'single' })])
  );
  listProjectRepos = vi.fn(async (projectId: string) =>
    swr([repo(projectId, { branchName: 'single' })])
  );
  listBranchesByProject = vi.fn().mockResolvedValue(swr(branchesOf('bulk')));
  listProjectReposByProject = vi.fn().mockResolvedValue(swr(reposOf('bulk')));
  ensureForRepos = vi.fn().mockResolvedValue(undefined);

  vi.doMock('../commands', () => ({
    listProjects,
    listBranchesForProject,
    listProjectRepos,
    listBranchesByProject,
    listProjectReposByProject,
    listReposForHome: vi.fn().mockResolvedValue([]),
    invalidateProjectBranchTimelines: vi.fn(),
    listTransferringProjects: vi.fn().mockResolvedValue([]),
  }));
  vi.doMock('../transport', () => ({
    listenToEvent: (event: string, callback: EventCallback) => {
      eventListeners.set(event, [...(eventListeners.get(event) ?? []), callback]);
      return () => {
        eventListeners.set(
          event,
          (eventListeners.get(event) ?? []).filter((cb) => cb !== callback)
        );
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
  vi.restoreAllMocks();
});

// ── Tests ──

describe('bulk hydration', () => {
  it('hydrates many projects with the two all-projects reads and none per project', async () => {
    const { store } = await loadedStore();
    expect(store.allProjectsHydrated).toBe(false);

    await store.ensureProjectsHydrated();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(1);
    // Plain reads: on the web they are served from the cache first.
    expect(listBranchesByProject).toHaveBeenCalledWith();
    expect(listProjectReposByProject).toHaveBeenCalledWith();
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'bulk' });
    expect(repoBranchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'bulk' });
    expect(store.allProjectsHydrated).toBe(true);
    expect(store.repoCountsByProject.get('p3')).toBe(1);
  });

  it('wakes the readers of each map once for the whole reply', async () => {
    const { store } = await loadedStore();
    const branches = await watch(() => store.branchesByProject);
    const repos = await watch(() => store.reposByProject);
    const hydrated = await watch(() => store.isProjectHydrated('p4'));
    const before = [branches.flush(), repos.flush(), hydrated.flush()];
    expect(before.map((seen) => seen.length)).toEqual([1, 1, 1]);

    await store.ensureProjectsHydrated();
    await settle();

    // One new map each, however many projects the reply carried.
    expect(branches.flush()).toEqual([store.branchesByProject]);
    expect(repos.flush()).toEqual([store.reposByProject]);
    expect(hydrated.flush()).toEqual([true]);
    expect(store.branchesByProject).not.toBe(before[0][0]);
    branches.stop();
    repos.stop();
    hydrated.stop();
  });

  it('asks for the badges of every repo in the reply at once', async () => {
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    expect(ensureForRepos).toHaveBeenCalledTimes(1);
    expect(ensureForRepos).toHaveBeenCalledWith(
      IDS.map((id) => ({ githubRepo: `org/${id}`, subpath: null }))
    );
  });

  it('reads one or two projects on their own', async () => {
    listProjects.mockResolvedValue(swr(['p1', 'p2'].map(project)));
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listProjectReposByProject).not.toHaveBeenCalled();
    expect(listBranchesForProject.mock.calls.map(([id]) => id)).toEqual(['p1', 'p2']);
    expect(branchNames(store, ['p1', 'p2'])).toEqual({ p1: 'single', p2: 'single' });
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('shares one bulk read between the grid, the board and the idle queue', async () => {
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValue(reply.promise);
    const { store, idle } = await loadedStore();

    const grid = store.ensureProjectsHydrated();
    const board = store.ensureProjectsHydrated();
    // The background hydration the load scheduled comes due meanwhile.
    idle.run();
    // The selected project, and the delete flow's safety check, want one of
    // them now: both join the read that already covers it.
    const selected = store.hydrateProject('p2');
    const guard = store.ensureProjectHydrated('p3');
    await settle();
    expect(store.isProjectHydrated('p2')).toBe(false);

    reply.resolve(swr(branchesOf('bulk')));
    await Promise.all([grid, board, selected, guard]);

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('leaves a project already being read on its own out of the bulk read', async () => {
    const single = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(single.promise);
    const { store } = await loadedStore();

    const selected = store.hydrateProject('p1');
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // The bulk reply lists p1 too, but p1 is the selected project's read.
    expect(branchNames(store)).toEqual({ p1: undefined, p2: 'bulk', p3: 'bulk', p4: 'bulk' });
    expect(store.isProjectHydrated('p1')).toBe(false);
    let swept = false;
    void sweep.then(() => (swept = true));
    await settle();
    expect(swept).toBe(false);

    single.resolve(swr([branch('p1', { branchName: 'single' })]));
    await Promise.all([selected, sweep]);

    expect(branchNames(store).p1).toBe('single');
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('marks every project settled when the bulk read fails, without reading each', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    listBranchesByProject.mockRejectedValue(new Error('store is locked'));
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    // A view gated on hydration must not hang, as for a one-project failure.
    expect(store.allProjectsHydrated).toBe(true);
    expect(branchNames(store)).toEqual({});
    // The repo reply is not applied without the branch reply it was read with.
    expect(store.reposByProject.size).toBe(0);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(error).toHaveBeenCalledWith(
      '[projectsData] Failed to hydrate 4 projects in bulk:',
      expect.any(Error)
    );
  });

  it('reads a project the reply does not list on its own', async () => {
    // p4 was created after the replies were read (or they are a cached copy
    // from before it existed): they say nothing about it.
    listBranchesByProject.mockResolvedValue(swr(branchesOf('bulk', ['p1', 'p2', 'p3'])));
    listProjectReposByProject.mockResolvedValue(swr(reposOf('bulk', ['p1', 'p2', 'p3'])));
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    expect(listBranchesForProject.mock.calls.map(([id]) => id)).toEqual(['p4']);
    expect(listProjectRepos.mock.calls.map(([id]) => id)).toEqual(['p4']);
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'single' });
    expect(repoBranchNames(store).p4).toBe('single');
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('skips a project that left the list while the read was out', async () => {
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValue(reply.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();

    // A soft revalidation (a view mounting) finds p3 gone.
    listProjects.mockResolvedValue(swr(['p1', 'p2', 'p4'].map(project)));
    await store.ensureLoaded();
    await settle();
    reply.resolve(swr(branchesOf('bulk')));
    await sweep;

    expect(store.branchesByProject.has('p3')).toBe(false);
    expect(store.reposByProject.has('p3')).toBe(false);
    expect(store.isProjectHydrated('p3')).toBe(false);
    expect(branchNames(store, ['p1', 'p2', 'p4'])).toEqual({ p1: 'bulk', p2: 'bulk', p4: 'bulk' });
    expect(store.allProjectsHydrated).toBe(true);
  });
});

describe('bulk hydration: load generation', () => {
  it('discards a bulk reply that lands after a newer load started', async () => {
    const stale = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValueOnce(stale.promise);
    const { store } = await loadedStore();
    store.startListeners();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // The change feed lost track: the list reloads under a new generation.
    emit('project-changed', { projectId: null });
    await settle();
    stale.resolve(swr(branchesOf('stale')));
    await sweep;

    // Nothing of the old load's reply is shown, and nothing is marked
    // hydrated on the strength of it.
    expect(branchNames(store)).toEqual({});
    expect(store.reposByProject.size).toBe(0);
    expect(IDS.map((id) => store.isProjectHydrated(id))).toEqual([false, false, false, false]);
    store.stopListeners();
  });

  it('keeps the new load’s reply when the old one lands after it', async () => {
    const stale = deferred<SwrResult<Record<string, Branch[]>>>();
    const staleRepos = deferred<SwrResult<Record<string, ProjectRepo[]>>>();
    listBranchesByProject
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValueOnce(swr(branchesOf('fresh')));
    listProjectReposByProject
      .mockReturnValueOnce(staleRepos.promise)
      .mockResolvedValueOnce(swr(reposOf('fresh')));
    const { store, idle } = await loadedStore();
    store.startListeners();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    emit('project-changed', { projectId: null });
    await settle();
    // The reload's own background hydration: a second bulk read.
    idle.run();
    await settle();
    expect(branchNames(store).p1).toBe('fresh');

    stale.resolve(swr(branchesOf('stale')));
    staleRepos.resolve(swr(reposOf('stale')));
    await sweep;
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(branchNames(store)).toEqual({ p1: 'fresh', p2: 'fresh', p3: 'fresh', p4: 'fresh' });
    expect(repoBranchNames(store)).toEqual({
      p1: 'fresh',
      p2: 'fresh',
      p3: 'fresh',
      p4: 'fresh',
    });
    store.stopListeners();
  });
});

describe('bulk hydration: read ordering per project', () => {
  it('keeps a project’s later refresh over the bulk reply, and applies the rest', async () => {
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValue(reply.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // Sent after the bulk read, lands before it: a repo was just added to p2.
    await expect(store.refreshProject('p2')).resolves.toBe(true);
    expect(branchNames(store).p2).toBe('single');

    reply.resolve(swr(branchesOf('bulk')));
    await sweep;

    // The bulk reply was read before p2's refresh: it must not put p2 back.
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    expect(repoBranchNames(store)).toEqual({
      p1: 'bulk',
      p2: 'single',
      p3: 'bulk',
      p4: 'bulk',
    });
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('refuses a one-project read sent before the bulk read that lands after it', async () => {
    const early = deferred<SwrResult<Branch[]>>();
    const earlyRepos = deferred<SwrResult<ProjectRepo[]>>();
    listBranchesForProject.mockReturnValueOnce(early.promise);
    listProjectRepos.mockReturnValueOnce(earlyRepos.promise);
    const { store } = await loadedStore();

    const refresh = store.refreshProject('p2');
    await store.ensureProjectsHydrated();
    expect(branchNames(store).p2).toBe('bulk');

    early.resolve(swr([branch('p2', { branchName: 'early' })]));
    earlyRepos.resolve(swr([repo('p2', { branchName: 'early' })]));
    await refresh;

    expect(branchNames(store).p2).toBe('bulk');
    expect(repoBranchNames(store).p2).toBe('bulk');
  });

  it('sets aside the entry of a project written locally meanwhile, and reads it again', async () => {
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValue(reply.promise);
    const reread = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(reread.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // The view paints its own write (a rename, a workspace status) to p2
    // while the bulk read, sent before it, is still out.
    store.setBranchesByProject(
      new Map(store.branchesByProject).set('p2', [branch('p2', { branchName: 'renamed' })])
    );
    reply.resolve(swr(branchesOf('bulk')));
    await sweep;

    // The reply may predate the write: p2 keeps what the view painted, the
    // other projects take the reply, and p2's list is read again.
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'renamed', p3: 'bulk', p4: 'bulk' });
    expect(store.isProjectHydrated('p2')).toBe(true);
    expect(listBranchesForProject.mock.calls).toEqual([['p2', { force: true }]]);

    reread.resolve(swr([branch('p2', { branchName: 'reread' })]));
    await settle();
    expect(branchNames(store).p2).toBe('reread');
  });

  it('lets the reply through after a local call that changed no branch', async () => {
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValue(reply.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // Callers rebuild the map even when no branch in it matched.
    store.setBranchesByProject(new Map(store.branchesByProject));
    reply.resolve(swr(branchesOf('bulk')));
    await sweep;

    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'bulk' });
    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('replays a branch change that arrived while the bulk read was out', async () => {
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValue(reply.promise);
    const { store } = await loadedStore();
    store.startListeners();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // The read in flight may have read p2 before this change.
    emit('branch-changed', { branchId: 'p2-b', projectId: 'p2' });
    await settle();
    expect(listBranchesForProject).not.toHaveBeenCalled();

    reply.resolve(swr(branchesOf('bulk')));
    await sweep;
    await settle();

    expect(listBranchesForProject.mock.calls).toEqual([['p2', { force: true }]]);
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    store.stopListeners();
  });
});

describe('bulk hydration: worktree paths', () => {
  it('keeps a worktree path the reply lacks, per project, as a one-project read does', async () => {
    const { store, idle } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();
    expect(store.branchesByProject.get('p1')?.[0].worktreePath).toBe('/wt/p1');

    // A reload reads the branches while their worktrees are being set up
    // again: p1 with no path, p2 with a new one, p3 now remote, p4 unchanged.
    listBranchesByProject.mockResolvedValue(
      swr({
        p1: [branch('p1', { worktreePath: null })],
        p2: [branch('p2', { worktreePath: '/wt/moved' })],
        p3: [branch('p3', { worktreePath: null, branchType: 'remote' })],
        p4: [branch('p4'), branch('p4', { id: 'p4-new', worktreePath: null })],
      })
    );
    emit('project-changed', { projectId: null });
    await settle();
    idle.run();
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    const paths = (id: string) => store.branchesByProject.get(id)?.map((b) => b.worktreePath);
    // Preserved over the stale null.
    expect(paths('p1')).toEqual(['/wt/p1']);
    // A populated path wins.
    expect(paths('p2')).toEqual(['/wt/moved']);
    // Never carried across a type change: the transfer removed the worktree.
    expect(paths('p3')).toEqual([null]);
    // The list takes the reply's shape: a new branch has no path to keep.
    expect(paths('p4')).toEqual(['/wt/p4', null]);
    store.stopListeners();
  });
});

describe('bulk hydration: cached reads (web)', () => {
  it('applies the cached replies, then the revalidated ones', async () => {
    const freshBranches = deferred<Record<string, Branch[]>>();
    const freshRepos = deferred<Record<string, ProjectRepo[]>>();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('cached'), freshBranches.promise));
    listProjectReposByProject.mockResolvedValue(swr(reposOf('cached'), freshRepos.promise));
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    // Painted from the cache without waiting on the network.
    expect(branchNames(store)).toEqual({
      p1: 'cached',
      p2: 'cached',
      p3: 'cached',
      p4: 'cached',
    });
    expect(repoBranchNames(store).p1).toBe('cached');
    expect(store.allProjectsHydrated).toBe(true);

    // The two revalidations land on their own, in either order.
    freshRepos.resolve(reposOf('fresh'));
    await settle();
    expect(repoBranchNames(store)).toEqual({
      p1: 'fresh',
      p2: 'fresh',
      p3: 'fresh',
      p4: 'fresh',
    });
    expect(branchNames(store).p1).toBe('cached');

    freshBranches.resolve(branchesOf('fresh'));
    await settle();
    expect(branchNames(store)).toEqual({ p1: 'fresh', p2: 'fresh', p3: 'fresh', p4: 'fresh' });
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('wakes the readers of the branch map once for the revalidated reply', async () => {
    const fresh = deferred<Record<string, Branch[]>>();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('cached'), fresh.promise));
    const { store } = await loadedStore();
    await store.ensureProjectsHydrated();
    const branches = await watch(() => store.branchesByProject);
    expect(branches.flush()).toHaveLength(1);

    fresh.resolve(branchesOf('fresh'));
    await settle();

    expect(branches.flush()).toEqual([store.branchesByProject]);
    branches.stop();
  });

  it('holds the revalidated reply to the same ordering as the cached one', async () => {
    const fresh = deferred<Record<string, Branch[]>>();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('cached'), fresh.promise));
    const reread = deferred<SwrResult<Branch[]>>();
    const { store } = await loadedStore();
    await store.ensureProjectsHydrated();

    // While the revalidation is out: p2 is refreshed, p3 is written locally.
    await expect(store.refreshProject('p2')).resolves.toBe(true);
    listBranchesForProject.mockReturnValueOnce(reread.promise);
    store.setBranchesByProject(
      new Map(store.branchesByProject).set('p3', [branch('p3', { branchName: 'renamed' })])
    );
    fresh.resolve(branchesOf('fresh'));
    await settle();

    // p2's refresh was sent after the bulk read, so it stays; p3's entry may
    // predate the write, so it is set aside and read again.
    expect(branchNames(store)).toEqual({
      p1: 'fresh',
      p2: 'single',
      p3: 'renamed',
      p4: 'fresh',
    });
    expect(listBranchesForProject).toHaveBeenLastCalledWith('p3', { force: true });
    reread.resolve(swr([branch('p3', { branchName: 'reread' })]));
    await settle();
    expect(branchNames(store).p3).toBe('reread');
  });

  it('keeps a worktree path the revalidated reply lacks', async () => {
    const fresh = deferred<Record<string, Branch[]>>();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('cached'), fresh.promise));
    const { store } = await loadedStore();
    await store.ensureProjectsHydrated();

    fresh.resolve({
      ...branchesOf('fresh'),
      p1: [branch('p1', { branchName: 'fresh', worktreePath: null })],
    });
    await settle();

    expect(store.branchesByProject.get('p1')?.[0]).toMatchObject({
      branchName: 'fresh',
      worktreePath: '/wt/p1',
    });
  });

  it('drops a revalidated reply that lands after a newer load started', async () => {
    const fresh = deferred<Record<string, Branch[]>>();
    const freshRepos = deferred<Record<string, ProjectRepo[]>>();
    listBranchesByProject.mockResolvedValueOnce(swr(branchesOf('cached'), fresh.promise));
    listProjectReposByProject.mockResolvedValueOnce(swr(reposOf('cached'), freshRepos.promise));
    const { store } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();

    emit('project-changed', { projectId: null });
    await settle();
    fresh.resolve(branchesOf('late'));
    freshRepos.resolve(reposOf('late'));
    await settle();

    expect(branchNames(store)).toEqual({
      p1: 'cached',
      p2: 'cached',
      p3: 'cached',
      p4: 'cached',
    });
    expect(repoBranchNames(store).p1).toBe('cached');
    store.stopListeners();
  });

  it('keeps the cached replies when their revalidation fails', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const fresh = deferred<Record<string, Branch[]>>();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('cached'), fresh.promise));
    const { store } = await loadedStore();
    await store.ensureProjectsHydrated();

    fresh.reject(new Error('offline'));
    await settle();

    expect(branchNames(store).p1).toBe('cached');
    expect(error).toHaveBeenCalledWith(
      '[projectsData] Failed to revalidate the branches of all projects:',
      expect.any(Error)
    );
  });

  it('reads a project a cached reply predates on its own', async () => {
    // The cached replies were written before p4 existed; the revalidated
    // ones list it.
    const fresh = deferred<Record<string, Branch[]>>();
    listBranchesByProject.mockResolvedValue(
      swr(branchesOf('cached', ['p1', 'p2', 'p3']), fresh.promise)
    );
    listProjectReposByProject.mockResolvedValue(swr(reposOf('cached', ['p1', 'p2', 'p3'])));
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();
    expect(branchNames(store).p4).toBe('single');

    // p4's own read was sent after the bulk read, so the revalidated reply
    // does not replace it.
    fresh.resolve(branchesOf('fresh'));
    await settle();
    expect(branchNames(store)).toEqual({ p1: 'fresh', p2: 'fresh', p3: 'fresh', p4: 'single' });
  });
});

describe('bulk hydration: reloads', () => {
  it('rehydrates every project with one bulk read after a hard reload', async () => {
    const { store, idle } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('reloaded')));
    listProjectReposByProject.mockResolvedValue(swr(reposOf('reloaded')));

    // An event that names no project is the hard reload (the feed's lag
    // recovery). One that names its project refetches that project alone:
    // see projectsData.projectChanged.component.test.ts.
    emit('project-changed', { projectId: null });
    await settle();
    // Painted views keep what they show until the idle step refreshes it.
    expect(branchNames(store).p1).toBe('bulk');
    expect(store.allProjectsHydrated).toBe(true);
    idle.run();
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual({
      p1: 'reloaded',
      p2: 'reloaded',
      p3: 'reloaded',
      p4: 'reloaded',
    });
    expect(repoBranchNames(store).p4).toBe('reloaded');
    store.stopListeners();
  });

  it('drops the idle step of a reload that a newer one replaced', async () => {
    const { store, idle } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();

    emit('project-changed', { projectId: null });
    await settle();
    emit('project-changed', { projectId: null });
    await settle();
    idle.run();
    await settle();

    // One bulk read for the two reloads, not one each.
    expect(listProjects).toHaveBeenCalledTimes(3);
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    store.stopListeners();
  });

  it('leaves out of the idle step what was fetched under the reload already', async () => {
    const { store, idle } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();

    emit('project-changed', { projectId: null });
    await settle();
    // The selected project and one more are read before the idle step runs.
    await store.hydrateProject('p1');
    await store.hydrateProject('p2');
    idle.run();
    await settle();

    // Two left: read on their own, and no second bulk read for them.
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls.map(([id]) => id)).toEqual(['p1', 'p2', 'p3', 'p4']);
    expect(branchNames(store)).toEqual({
      p1: 'single',
      p2: 'single',
      p3: 'single',
      p4: 'single',
    });
    store.stopListeners();
  });

  it('rehydrates what was painted with one bulk read on cache-stale', async () => {
    const { store } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('reloaded')));

    windowTarget.dispatchEvent(new Event('cache-stale'));
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store).p3).toBe('reloaded');
    store.stopListeners();
  });
});
