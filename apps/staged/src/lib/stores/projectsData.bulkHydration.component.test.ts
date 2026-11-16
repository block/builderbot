// The bulk hydration path of projectsDataStore: many projects hydrated with
// two all-projects reads and applied once. What it must keep from the
// one-project path is the subject here: the load generation check, each
// project's read ordering against a later read or a local write of that
// project, the worktree-preserving merge, and the web build's cached read
// followed by its revalidated one.
//
// Then the paths that used to fall back to one read per project, and the
// recovery from a bulk read that fails: a retry, a re-read when a view next
// asks, and a hard reload on the next project-changed.
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

/** Let every reply already resolved land, without letting a timer fire:
 *  the store's per-frame flushes are timers here, and stay pending. */
async function microtasks(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
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
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    listBranchesByProject.mockRejectedValue(new Error('store is locked'));
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    // A view gated on hydration must not hang, as for a one-project failure.
    // What recovers the lists is under "a read that fails" below.
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

describe('bulk hydration: a read that fails', () => {
  const failure = () => new Error('store is locked');
  const ALL_BULK = { p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'bulk' };

  /** Silence what the store reports, and hand the reports back. */
  function quiet() {
    return {
      warn: vi.spyOn(console, 'warn').mockImplementation(() => {}),
      error: vi.spyOn(console, 'error').mockImplementation(() => {}),
    };
  }

  /** A loaded store whose first bulk hydration failed, retry included. */
  async function storeAfterFailedHydration() {
    listBranchesByProject.mockRejectedValueOnce(failure()).mockRejectedValueOnce(failure());
    const loaded = await loadedStore();
    await loaded.store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(branchNames(loaded.store)).toEqual({});
    return loaded;
  }

  it('sends the two reads once more, and applies the second reply', async () => {
    const { warn, error } = quiet();
    listBranchesByProject.mockRejectedValueOnce(failure());
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_BULK);
    expect(repoBranchNames(store)).toEqual(ALL_BULK);
    expect(store.allProjectsHydrated).toBe(true);
    expect(warn).toHaveBeenCalledWith(
      '[projectsData] Reading 4 projects in bulk failed, trying once more:',
      expect.any(Error)
    );
    expect(error).not.toHaveBeenCalled();
    // Their lists landed, so nothing is left to recover.
    await store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
  });

  it('retries when it is the repo read that failed', async () => {
    quiet();
    listProjectReposByProject.mockRejectedValueOnce(failure());
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    expect(repoBranchNames(store)).toEqual(ALL_BULK);
  });

  it('retries a forced read as a forced read', async () => {
    quiet();
    const { store } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();
    listBranchesByProject.mockRejectedValueOnce(failure());

    emit('branch-changed', { branchId: null, projectId: null });
    await settle();

    expect(listBranchesByProject.mock.calls.slice(1)).toEqual([
      [{ force: true }],
      [{ force: true }],
    ]);
    expect(listProjectReposByProject.mock.calls.slice(1)).toEqual([
      [{ force: true }],
      [{ force: true }],
    ]);
    store.stopListeners();
  });

  it('orders the second reply as sent when it was, after a refresh sent meanwhile', async () => {
    quiet();
    const first = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValueOnce(first.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // Sent after the first bulk read, and landed before the second is sent.
    await store.refreshProject('p1');
    expect(branchNames(store).p1).toBe('single');
    first.reject(failure());
    await sweep;

    // The second read was sent after that refresh, so its list for p1 is the
    // newer one. Under the first read's place in the order it would be
    // refused as older.
    expect(branchNames(store)).toEqual(ALL_BULK);
  });

  it('gives up after the second failure, and does not read each project', async () => {
    const { error } = quiet();
    const { store } = await storeAfterFailedHydration();

    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(store.allProjectsHydrated).toBe(true);
    expect(store.reposByProject.size).toBe(0);
    expect(error).toHaveBeenCalledWith(
      '[projectsData] Failed to hydrate 4 projects in bulk:',
      expect.any(Error)
    );
  });

  it('reads the projects again when a view next asks for them', async () => {
    quiet();
    const { store } = await storeAfterFailedHydration();

    // The grid and the board ask on mount and on every list change.
    await store.ensureProjectsHydrated();

    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_BULK);
    expect(repoBranchNames(store)).toEqual(ALL_BULK);
    // Read now, so the next ask is a no-op again.
    await store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
  });

  it('reads one project again for a caller that must not act on the fallbacks', async () => {
    quiet();
    const { store } = await storeAfterFailedHydration();

    // The project-delete safety check.
    await store.ensureProjectHydrated('p2');

    expect(listBranchesForProject.mock.calls).toEqual([['p2']]);
    expect(branchNames(store)).toEqual({ p2: 'single' });
    // p2 is read now. The other three are still owed their lists.
    await store.ensureProjectHydrated('p2');
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    await store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
  });

  it('counts a refresh of the project as the read it was owed', async () => {
    quiet();
    const { store } = await storeAfterFailedHydration();

    await store.refreshProject('p3');
    listBranchesForProject.mockClear();
    await store.ensureProjectHydrated('p3');

    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual({ p3: 'single' });
  });

  it('reloads everything on the next project-changed, though it names one project', async () => {
    quiet();
    const { store, idle } = await storeAfterFailedHydration();
    store.startListeners();

    emit('project-changed', { projectId: 'p1' });
    await settle();
    idle.run();
    await settle();

    // A hard reload: every project is hydrated again, in bulk. A targeted
    // one would have read p1 on its own and left the rest on empty lists.
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_BULK);
    // With the lists back, the next event is targeted again.
    emit('project-changed', { projectId: 'p1' });
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    expect(listBranchesForProject.mock.calls).toEqual([['p1', { force: true }]]);
    store.stopListeners();
  });

  it('does not wake the views gated on hydration when a read fails again', async () => {
    quiet();
    listBranchesByProject.mockRejectedValue(failure());
    const { store } = await loadedStore();
    const hydrated = await watch(() => store.isProjectHydrated('p1'));
    expect(hydrated.flush()).toEqual([false]);

    await store.ensureProjectsHydrated();
    expect(hydrated.flush()).toEqual([true]);

    // A view that re-asks whenever the hydrated map changes would otherwise
    // read, fail and be woken again, without end.
    await store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(4);
    expect(hydrated.flush()).toEqual([]);
    hydrated.stop();
  });

  it('does not wake them for one project’s read that fails again either', async () => {
    quiet();
    listProjects.mockResolvedValue(swr(['p1', 'p2'].map(project)));
    listBranchesForProject.mockRejectedValue(failure());
    const { store } = await loadedStore();
    await store.ensureProjectHydrated('p1');
    const hydrated = await watch(() => store.isProjectHydrated('p1'));
    expect(hydrated.flush()).toEqual([true]);

    await store.ensureProjectHydrated('p1');

    expect(listBranchesForProject).toHaveBeenCalledTimes(2);
    expect(hydrated.flush()).toEqual([]);
    hydrated.stop();
  });

  it('is not retried, and leaves no mark, once a newer load has started', async () => {
    quiet();
    const first = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValueOnce(first.promise);
    const { store, idle } = await loadedStore();
    store.startListeners();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    // A hard reload replaces the load that read belongs to, and hydrates
    // every project itself.
    emit('project-changed', { projectId: null });
    await settle();
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    first.reject(failure());
    await sweep;
    await settle();

    // No second try for the replaced load.
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(branchNames(store)).toEqual(ALL_BULK);
    store.stopListeners();
  });

  it('leaves no mark when its second try fails after a newer load has started', async () => {
    quiet();
    const retry = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockRejectedValueOnce(failure()).mockReturnValueOnce(retry.promise);
    const { store, idle } = await loadedStore();
    store.startListeners();
    const sweep = store.ensureProjectsHydrated();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);

    emit('project-changed', { projectId: null });
    await settle();
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    retry.reject(failure());
    await sweep;
    await settle();

    // The newer load read every project: none is owed a read,
    await store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    // and the next event is still targeted.
    emit('project-changed', { projectId: 'p1' });
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(3);
    expect(listBranchesForProject.mock.calls).toEqual([['p1', { force: true }]]);
    store.stopListeners();
  });

  it('reads on its own a project the backend left out, and owes a read to it alone', async () => {
    quiet();
    // The backend could not read p4, so the reply does not list it.
    listBranchesByProject.mockResolvedValue(swr(branchesOf('bulk', ['p1', 'p2', 'p3'])));
    listBranchesForProject.mockRejectedValueOnce(new Error('database disk image is malformed'));
    const { store } = await loadedStore();
    store.startListeners();

    await store.ensureProjectsHydrated();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls).toEqual([['p4']]);
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: undefined });
    expect(store.allProjectsHydrated).toBe(true);

    // One project failed, not the reply: the next event stays targeted,
    emit('project-changed', { projectId: 'p1' });
    await settle();
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls.slice(1)).toEqual([['p1', { force: true }]]);
    // and the next ask reads that one project, not all of them.
    await store.ensureProjectsHydrated();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls.slice(2)).toEqual([['p4']]);
    expect(branchNames(store).p4).toBe('single');
    await store.ensureProjectsHydrated();
    expect(listBranchesForProject).toHaveBeenCalledTimes(3);
    store.stopListeners();
  });

  it('reads again a project whose own hydration failed, and no other', async () => {
    quiet();
    listProjects.mockResolvedValue(swr(['p1', 'p2'].map(project)));
    listBranchesForProject.mockRejectedValueOnce(failure());
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();
    expect(store.allProjectsHydrated).toBe(true);
    expect(branchNames(store, ['p1', 'p2'])).toEqual({ p1: undefined, p2: 'single' });
    listBranchesForProject.mockClear();

    await store.ensureProjectHydrated('p2');
    expect(listBranchesForProject).not.toHaveBeenCalled();
    await store.ensureProjectHydrated('p1');
    expect(listBranchesForProject.mock.calls).toEqual([['p1']]);
    expect(branchNames(store, ['p1', 'p2'])).toEqual({ p1: 'single', p2: 'single' });
    await store.ensureProjectHydrated('p1');
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
  });
});

describe('bulk hydration: the feed’s lag recovery', () => {
  // What the change feed sends when it has lost track: every event with
  // every id null, project-changed first (store_events.rs, lag_flush_events).
  const LAG = { branchId: null, projectId: null };
  const ALL_RECOVERED = { p1: 'recovered', p2: 'recovered', p3: 'recovered', p4: 'recovered' };

  /** A store with every project hydrated, whose next bulk reply is `recovered`. */
  async function hydratedStore() {
    const loaded = await loadedStore();
    loaded.store.startListeners();
    await loaded.store.ensureProjectsHydrated();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('recovered')));
    listProjectReposByProject.mockResolvedValue(swr(reposOf('recovered')));
    return loaded;
  }

  it('reads every hydrated project again with one forced bulk read, and none per project', async () => {
    const { store } = await hydratedStore();

    emit('branch-changed', LAG);
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    // Forced: the lists held are known to be out of date.
    expect(listBranchesByProject).toHaveBeenLastCalledWith({ force: true });
    expect(listProjectReposByProject).toHaveBeenLastCalledWith({ force: true });
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    expect(repoBranchNames(store)).toEqual(ALL_RECOVERED);
    store.stopListeners();
  });

  it('wakes the readers of the branch map once, not once per project', async () => {
    const { store } = await hydratedStore();
    const branches = await watch(() => store.branchesByProject);
    branches.flush();

    emit('branch-changed', LAG);
    await settle();

    expect(branches.flush()).toEqual([store.branchesByProject]);
    branches.stop();
    store.stopListeners();
  });

  it('is one bulk hydration together with the hard reload that comes with it', async () => {
    const { store, idle } = await hydratedStore();

    emit('project-changed', { projectId: null });
    emit('branch-changed', LAG);
    await settle();
    idle.run();
    await settle();

    // The recovery's read covered every project under the new load, so the
    // reload's idle step has nothing left to read.
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    store.stopListeners();
  });

  it('leaves alone a hydration that was sent after it arrived', async () => {
    const { store, idle } = await hydratedStore();
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValueOnce(reply.promise);

    emit('project-changed', { projectId: null });
    emit('branch-changed', LAG);
    // The reload's list lands and its idle step runs before the frame that
    // flushes the branch event: its hydration is out, sent after the event.
    await microtasks();
    idle.run();
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    await settle();

    // That hydration reads what the recovery asks for. No second one.
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    reply.resolve(swr(branchesOf('recovered')));
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    store.stopListeners();
  });

  it('reads again what a hydration sent before it arrived may have missed', async () => {
    const first = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValueOnce(first.promise);
    const { store } = await loadedStore();
    store.startListeners();
    const sweep = store.ensureProjectsHydrated();
    await settle();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('recovered')));
    listProjectReposByProject.mockResolvedValue(swr(reposOf('recovered')));

    emit('branch-changed', LAG);
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).toHaveBeenLastCalledWith({ force: true });
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    // The older reply lands last, and loses to the one sent after it.
    first.resolve(swr(branchesOf('before the lag')));
    await sweep;
    await settle();
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    store.stopListeners();
  });

  it('reads one or two projects on their own', async () => {
    listProjects.mockResolvedValue(swr(['p1', 'p2'].map(project)));
    const { store } = await loadedStore();
    store.startListeners();
    await store.ensureProjectsHydrated();
    listBranchesForProject.mockClear();

    emit('branch-changed', LAG);
    await settle();

    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listBranchesForProject.mock.calls).toEqual([
      ['p1', { force: true }],
      ['p2', { force: true }],
    ]);
    store.stopListeners();
  });

  it('does not read projects that were never fetched', async () => {
    const { store, idle } = await loadedStore();
    store.startListeners();

    emit('branch-changed', LAG);
    await settle();

    // Their first hydration is yet to be sent, and reads what there is then.
    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listBranchesForProject).not.toHaveBeenCalled();
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesByProject).toHaveBeenLastCalledWith();
    store.stopListeners();
  });

  it('reads projects that a refresh in flight is about to mark hydrated', async () => {
    const { store } = await loadedStore();
    store.startListeners();
    // Sent before the recovery arrived, and marks its project hydrated when
    // it lands: what it read is in doubt as well.
    const held = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValue(held.promise);
    const refreshes = ['p1', 'p2', 'p3'].map((id) => store.refreshProject(id));
    await settle();
    listBranchesByProject.mockResolvedValue(swr(branchesOf('recovered')));

    emit('branch-changed', LAG);
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesByProject).toHaveBeenLastCalledWith({ force: true });
    held.resolve(swr([]));
    await Promise.all(refreshes);
    await settle();
    // The older replies land last, and lose to the read sent after them.
    expect(branchNames(store)).toEqual({
      p1: 'recovered',
      p2: 'recovered',
      p3: 'recovered',
      p4: undefined,
    });
    store.stopListeners();
  });

  it('leaves alone one project’s hydration that was sent after it arrived', async () => {
    listProjects.mockResolvedValue(swr(['p1', 'p2'].map(project)));
    const { store, idle } = await loadedStore();
    store.startListeners();
    const reply = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(reply.promise);

    // Nothing is hydrated yet. The reload's idle step sends the first
    // hydration after the recovery arrived, and before its flush.
    emit('project-changed', { projectId: null });
    emit('branch-changed', LAG);
    await microtasks();
    idle.run();
    expect(listBranchesForProject.mock.calls).toEqual([['p1']]);
    await settle();
    reply.resolve(swr([branch('p1', { branchName: 'single' })]));
    await settle();

    // It read what the recovery asks for: no forced read follows it, and
    // the project still waiting its turn is read once, in its turn.
    expect(listBranchesForProject.mock.calls).toEqual([['p1']]);
    idle.run();
    await settle();
    expect(listBranchesForProject.mock.calls).toEqual([['p1'], ['p2']]);
    expect(listBranchesByProject).not.toHaveBeenCalled();
    store.stopListeners();
  });

  it('is one bulk hydration when its flush runs before the reload’s list lands', async () => {
    const { store, idle } = await hydratedStore();
    const list = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(list.promise);

    emit('project-changed', { projectId: null });
    emit('branch-changed', LAG);
    await settle();
    // Sent for the load the reload started, while its list is still out.
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    list.resolve(swr(IDS.map(project)));
    await settle();
    idle.run();
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    expect(store.allProjectsHydrated).toBe(true);
    store.stopListeners();
  });

  it('reads the hydrated projects and leaves the rest to their first hydration', async () => {
    const { store } = await loadedStore();
    store.startListeners();
    for (const id of ['p1', 'p2', 'p3']) await store.hydrateProject(id);
    listBranchesByProject.mockResolvedValue(swr(branchesOf('recovered')));

    emit('branch-changed', LAG);
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    // The reply carries p4 as well, but the read was not for it.
    expect(branchNames(store)).toEqual({
      p1: 'recovered',
      p2: 'recovered',
      p3: 'recovered',
      p4: undefined,
    });
    expect(store.isProjectHydrated('p4')).toBe(false);
    store.stopListeners();
  });

  it('skips a project that is being deleted', async () => {
    const { store } = await hydratedStore();
    store.projectDeleteStarted('p2', 'p2');

    emit('branch-changed', LAG);
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(branchNames(store)).toEqual({ ...ALL_RECOVERED, p2: 'bulk' });
    expect(listBranchesForProject).not.toHaveBeenCalled();
    store.stopListeners();
  });

  it('covers the events of the same burst that name a project it reads', async () => {
    const { store } = await hydratedStore();

    emit('branch-changed', { branchId: 'p1-b', projectId: 'p1' });
    emit('branch-changed', LAG);
    emit('branch-changed', { branchId: 'p3-b', projectId: null });
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual(ALL_RECOVERED);
    store.stopListeners();
  });

  it('still replays a named event for a project it left to a newer hydration', async () => {
    const { store, idle } = await hydratedStore();
    const reply = deferred<SwrResult<Record<string, Branch[]>>>();
    listBranchesByProject.mockReturnValueOnce(reply.promise);

    emit('project-changed', { projectId: null });
    emit('branch-changed', LAG);
    await microtasks();
    idle.run();
    // Sent after the recovery arrived, but perhaps before this change.
    emit('branch-changed', { branchId: 'p1-b', projectId: 'p1' });
    await settle();

    // No second bulk read. p1 was hydrated before the reload, so its refetch
    // is sent at once, as for any named event,
    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject.mock.calls).toEqual([['p1', { force: true }]]);
    reply.resolve(swr(branchesOf('recovered')));
    await settle();

    // and wins by send order over the hydration that lands after it.
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(branchNames(store)).toEqual({ ...ALL_RECOVERED, p1: 'single' });
    store.stopListeners();
  });
});

describe('bulk hydration: projects a soft reload discovers', () => {
  const NEW_IDS = ['n1', 'n2', 'n3'];

  /** A hydrated store whose next project list also carries `discovered`. */
  async function storeAboutToDiscover(discovered: string[]) {
    const loaded = await loadedStore();
    await loaded.store.ensureProjectsHydrated();
    const ids = [...discovered, ...IDS];
    listProjects.mockResolvedValue(swr(ids.map(project)));
    listBranchesByProject.mockResolvedValue(swr(branchesOf('later', ids)));
    listProjectReposByProject.mockResolvedValue(swr(reposOf('later', ids)));
    return loaded;
  }

  it('hydrates them together, with one bulk read in one idle step', async () => {
    const { store, idle } = await storeAboutToDiscover(NEW_IDS);

    // A loaded store revalidates softly: the list is read again, and what is
    // hydrated or in flight is kept.
    await store.ensureLoaded();
    await settle();
    expect(store.projects.map((p) => p.id)).toEqual([...NEW_IDS, ...IDS]);
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(store.allProjectsHydrated).toBe(false);
    idle.run();
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(2);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).toHaveBeenLastCalledWith();
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store, NEW_IDS)).toEqual({ n1: 'later', n2: 'later', n3: 'later' });
    expect(store.allProjectsHydrated).toBe(true);
    // The read was for the new projects: the others keep the lists they had.
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'bulk' });
  });

  it('wakes the readers of the branch map once for all of them', async () => {
    const { store, idle } = await storeAboutToDiscover(NEW_IDS);
    await store.ensureLoaded();
    await settle();
    const branches = await watch(() => store.branchesByProject);
    branches.flush();

    idle.run();
    await settle();

    expect(branches.flush()).toEqual([store.branchesByProject]);
    branches.stop();
  });

  it('reads one or two of them on their own', async () => {
    const { store, idle } = await storeAboutToDiscover(['n1', 'n2']);

    await store.ensureLoaded();
    await settle();
    expect(listBranchesForProject).not.toHaveBeenCalled();
    idle.run();
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls).toEqual([['n1'], ['n2']]);
    expect(branchNames(store, ['n1', 'n2'])).toEqual({ n1: 'single', n2: 'single' });
  });

  it('does not read one that was deleted before the idle step ran', async () => {
    const { store, idle } = await storeAboutToDiscover(NEW_IDS);
    await store.ensureLoaded();
    await settle();

    // n3 is gone again by the next reload, and the step has not run yet.
    listProjects.mockResolvedValue(swr(['n1', 'n2', ...IDS].map(project)));
    await store.ensureLoaded();
    await settle();
    expect(store.projects.map((p) => p.id)).toEqual(['n1', 'n2', ...IDS]);
    idle.run();
    await settle();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls).toEqual([['n1'], ['n2']]);
  });
});

describe('bulk hydration: refresh', () => {
  it('covers every project with one bulk hydration when the store is partly hydrated', async () => {
    const { store, idle } = await loadedStore();
    // Only the selected project has been read.
    await store.hydrateProject('p1');
    listBranchesForProject.mockClear();
    listProjectRepos.mockClear();

    await store.refresh();

    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'bulk', p3: 'bulk', p4: 'bulk' });
    expect(store.allProjectsHydrated).toBe(true);
    // The idle step the reload queued finds them all fetched.
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
  });

  it('leaves a store with nothing hydrated to the idle step', async () => {
    const { store, idle } = await loadedStore();

    await store.refresh();

    // Nothing is painted, so nothing is read ahead of the idle queue.
    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listBranchesForProject).not.toHaveBeenCalled();
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('reads only the hydrated project when there are too few projects to read in bulk', async () => {
    listProjects.mockResolvedValue(swr(['p1', 'p2'].map(project)));
    const { store } = await loadedStore();
    await store.hydrateProject('p1');
    listBranchesForProject.mockClear();

    await store.refresh();

    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listBranchesForProject.mock.calls).toEqual([['p1']]);
    expect(store.isProjectHydrated('p2')).toBe(false);
  });
});

describe('bulk hydration: badges', () => {
  it('asks for each repo’s badge once, however many projects attach it', async () => {
    listProjectReposByProject.mockResolvedValue(
      swr({
        p1: [repo('p1', { githubRepo: 'org/shared' })],
        p2: [
          repo('p2', { githubRepo: 'org/shared' }),
          repo('p2', { id: 'p2-r2', githubRepo: 'org/shared', subpath: 'apps/web' }),
        ],
        p3: [repo('p3', { githubRepo: 'org/shared', subpath: 'apps/web' })],
        p4: [repo('p4', { githubRepo: 'org/other' })],
      })
    );
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    expect(ensureForRepos).toHaveBeenCalledTimes(1);
    expect(ensureForRepos).toHaveBeenCalledWith([
      { githubRepo: 'org/shared', subpath: null },
      { githubRepo: 'org/shared', subpath: 'apps/web' },
      { githubRepo: 'org/other', subpath: null },
    ]);
    // Every project still gets its own repo list.
    expect(store.reposByProject.get('p2')?.length).toBe(2);
  });

  it('tells apart repos whose names and subpaths run together the same', async () => {
    listProjectReposByProject.mockResolvedValue(
      swr({
        p1: [repo('p1', { githubRepo: 'org/app', subpath: 'web' })],
        p2: [repo('p2', { githubRepo: 'org/appweb' })],
        p3: [repo('p3', { githubRepo: 'org/app' })],
        p4: [repo('p4', { githubRepo: 'org/app', subpath: '' })],
      })
    );
    const { store } = await loadedStore();

    await store.ensureProjectsHydrated();

    // A missing subpath and an empty one are the same repo root.
    expect(ensureForRepos).toHaveBeenCalledWith([
      { githubRepo: 'org/app', subpath: 'web' },
      { githubRepo: 'org/appweb', subpath: null },
      { githubRepo: 'org/app', subpath: null },
    ]);
  });
});
