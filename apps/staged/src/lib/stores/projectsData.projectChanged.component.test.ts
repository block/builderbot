// The targeted project-changed path of projectsDataStore. An event that
// names its project reloads the list softly and refetches that one project,
// where every event used to reload everything under a new load generation.
// A fetched list keeps the object of every project it leaves unchanged, and
// is not applied when it changes nothing.
//
// The subject is what the hard reload used to guarantee and the soft path
// must still deliver: the delete, create, rename, repo attach and detach,
// branch move and transfer flows; an event for a project the store has not
// seen, or that is already deleted; and events racing a load or a hydration
// in flight.
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
const FORCED = { force: true };

function project(id: string, overrides: Partial<Project> = {}): Project {
  return {
    id,
    name: id,
    githubRepo: `org/${id}`,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
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

/** The backend's project rows. Every read answers with new objects, as a
 *  real reply does, so an object the store still holds was kept by it. */
let rows: Project[];
let listProjects: ReturnType<typeof vi.fn>;
let listBranchesForProject: ReturnType<typeof vi.fn>;
let listProjectRepos: ReturnType<typeof vi.fn>;
let listBranchesByProject: ReturnType<typeof vi.fn>;
let listProjectReposByProject: ReturnType<typeof vi.fn>;
let ensureForRepos: ReturnType<typeof vi.fn>;
let badgeLoadAll: ReturnType<typeof vi.fn>;
let eventListeners: Map<string, EventCallback[]>;

function freshRows(): Project[] {
  return rows.map((row) => ({ ...row }));
}

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
 * Count how often the readers of `read()` are woken, the way a card's
 * $derived or a template re-runs. Imported after the store, so both share
 * the module graph (and with it the Svelte runtime) that beforeEach's
 * resetModules started. `wakes()` runs pending effects and returns the
 * number of runs since the last call; the first run, on subscribing, is not
 * counted.
 */
async function watch(read: () => unknown): Promise<{ wakes: () => number }> {
  const { readReactively } = await import('./testStubs/readReactively.svelte');
  const { flushSync } = await import('svelte');
  let runs = 0;
  readReactively(() => {
    read();
    runs++;
  });
  flushSync();
  runs = 0;
  return {
    wakes: () => {
      flushSync();
      const since = runs;
      runs = 0;
      return since;
    },
  };
}

const READ_MOCKS = () => [
  listProjects,
  listBranchesForProject,
  listProjectRepos,
  listBranchesByProject,
  listProjectReposByProject,
  ensureForRepos,
  badgeLoadAll,
];

/** A loaded store, listening, with the idle queue held: nothing hydrated. */
async function loadedStore(): Promise<{ store: Store; idle: { run: () => void } }> {
  const idle = holdIdleQueue();
  const store = await importStore();
  store.startListeners();
  await store.ensureLoaded();
  return { store, idle };
}

/**
 * A loaded store, listening, with every project hydrated in bulk (lists
 * named `bulk`) and the call history cleared, so a test counts only the
 * reads its own events cause.
 */
async function hydratedStore(): Promise<{ store: Store; idle: { run: () => void } }> {
  const { store, idle } = await loadedStore();
  await store.ensureProjectsHydrated();
  await settle();
  for (const mock of READ_MOCKS()) mock.mockClear();
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

function projectIds(store: Store): string[] {
  return store.projects.map((p) => p.id);
}

beforeEach(() => {
  vi.resetModules();
  // As in projectsData.component.test.ts: the store's per-frame flushes fall
  // back to setTimeout, which the tests' ticks wait on.
  vi.stubGlobal('requestAnimationFrame', undefined);
  vi.stubGlobal('window', new EventTarget());

  eventListeners = new Map();
  rows = IDS.map((id) => project(id));
  listProjects = vi.fn(async () => swr(freshRows()));
  // The one-project reads answer with lists named `single`, so a test can
  // tell which read a project's list came from.
  listBranchesForProject = vi.fn(async (projectId: string) =>
    swr([branch(projectId, { branchName: 'single' })])
  );
  listProjectRepos = vi.fn(async (projectId: string) =>
    swr([repo(projectId, { branchName: 'single' })])
  );
  listBranchesByProject = vi.fn(async () =>
    swr(Object.fromEntries(rows.map(({ id }) => [id, [branch(id)]])))
  );
  listProjectReposByProject = vi.fn(async () =>
    swr(Object.fromEntries(rows.map(({ id }) => [id, [repo(id)]])))
  );
  ensureForRepos = vi.fn().mockResolvedValue(undefined);
  badgeLoadAll = vi.fn().mockResolvedValue(undefined);

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
  vi.restoreAllMocks();
});

// ── The reload an event starts ──

describe('project-changed: which reload', () => {
  it('reloads the list and refetches only the project the event names', async () => {
    const { store, idle } = await hydratedStore();

    emit('project-changed', { projectId: 'p2' });
    await settle();
    idle.run();
    await settle();

    // Four reads, whatever the project count: the list, the badges, and the
    // named project's branches and repos.
    expect(listProjects.mock.calls).toEqual([[FORCED]]);
    expect(badgeLoadAll.mock.calls).toEqual([[FORCED]]);
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);
    expect(listProjectRepos.mock.calls).toEqual([['p2', FORCED]]);
    // No other project is hydrated again.
    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listProjectReposByProject).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    expect(repoBranchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('reads nothing of the named project until the list has landed', async () => {
    const reload = deferred<SwrResult<Project[]>>();
    const { store } = await hydratedStore();
    listProjects.mockReturnValueOnce(reload.promise);

    emit('project-changed', { projectId: 'p2' });
    await settle();
    // The list is what says whether p2 is still there to read.
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();

    reload.resolve(swr(freshRows()));
    await settle();
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);
    expect(branchNames(store).p2).toBe('single');
  });

  it.each([
    ['names no project', { projectId: null }],
    ['carries no project id at all', {}],
  ])('keeps the hard reload for an event that %s', async (_, payload) => {
    const { store, idle } = await hydratedStore();
    listBranchesByProject.mockImplementation(async () =>
      swr(Object.fromEntries(IDS.map((id) => [id, [branch(id, { branchName: 'reloaded' })]])))
    );

    emit('project-changed', payload);
    await settle();
    idle.run();
    await settle();

    // Every project is hydrated again, in bulk, and none on its own.
    expect(listProjects.mock.calls).toEqual([[FORCED]]);
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listProjectReposByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(branchNames(store)).toEqual({
      p1: 'reloaded',
      p2: 'reloaded',
      p3: 'reloaded',
      p4: 'reloaded',
    });
  });

  it('discards an apply in flight on the hard reload, and keeps it on the targeted one', async () => {
    const { store } = await hydratedStore();

    // A hard reload starts a new generation: a refresh sent under the old
    // one reports that it was overtaken.
    const overtaken = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(overtaken.promise);
    const refreshedAcrossHard = store.refreshProject('p1');
    emit('project-changed', { projectId: null });
    await settle();
    overtaken.resolve(swr([branch('p1', { branchName: 'old-generation' })]));
    expect(await refreshedAcrossHard).toBe(false);
    expect(branchNames(store).p1).toBe('bulk');

    // A targeted reload keeps the generation: a refresh of another project
    // in flight is applied.
    const kept = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(kept.promise);
    const refreshedAcrossSoft = store.refreshProject('p1');
    emit('project-changed', { projectId: 'p3' });
    await settle();
    kept.resolve(swr([branch('p1', { branchName: 'kept' })]));
    expect(await refreshedAcrossSoft).toBe(true);
    expect(branchNames(store).p1).toBe('kept');
  });

  it('does not fetch a project that is listed but was never hydrated', async () => {
    const { store, idle } = await loadedStore();

    emit('project-changed', { projectId: 'p2' });
    await settle();

    // Nothing of p2 was painted, so there is nothing to repair, and reading
    // it here would hydrate it apart from the rest.
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(store.isProjectHydrated('p2')).toBe(false);

    // The first load's idle step still hydrates everything, in bulk.
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('logs a refetch that fails and keeps the lists shown', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { store } = await hydratedStore();
    listBranchesForProject.mockRejectedValueOnce(new Error('offline'));
    listProjectRepos.mockRejectedValueOnce(new Error('offline'));

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(branchNames(store).p2).toBe('bulk');
    expect(repoBranchNames(store).p2).toBe('bulk');
    expect(store.isProjectHydrated('p2')).toBe(true);
    expect(error).toHaveBeenCalledWith(
      '[projectsData] Failed to refetch branches for project p2:',
      expect.any(Error)
    );
    expect(error).toHaveBeenCalledWith(
      '[projectsData] Failed to refetch repos for project p2:',
      expect.any(Error)
    );
  });
});

// ── Identity ──

describe('reuseUnchangedProjects', () => {
  async function reuse() {
    const { reuseUnchangedProjects } = await import('./projectsData.svelte');
    return reuseUnchangedProjects;
  }

  it('returns the list shown when the fetched one changes nothing', async () => {
    const reuseUnchangedProjects = await reuse();
    const current = IDS.map((id) => project(id));

    expect(reuseUnchangedProjects(current, freshRows())).toBe(current);
    expect(reuseUnchangedProjects([], [])).toEqual([]);
  });

  it('keeps the unchanged objects and takes the fetched one for a changed project', async () => {
    const reuseUnchangedProjects = await reuse();
    const current = IDS.map((id) => project(id));
    const fetched = freshRows();
    fetched[1] = project('p2', { name: 'renamed' });

    const next = reuseUnchangedProjects(current, fetched);

    expect(next).not.toBe(current);
    expect(next[0]).toBe(current[0]);
    expect(next[1]).toBe(fetched[1]);
    expect(next[2]).toBe(current[2]);
    expect(next[3]).toBe(current[3]);
  });

  it.each<[string, Partial<Project>]>([
    ['name', { name: 'other' }],
    ['repo', { githubRepo: null }],
    ['location', { location: 'remote' }],
    ['subpath', { subpath: 'apps/x' }],
    ['update time', { updatedAt: 1 }],
    ['creator, absent from the object shown', { createdBy: 'session:s1' }],
  ])('takes the fetched object when only the %s differs', async (_, change) => {
    const reuseUnchangedProjects = await reuse();
    const current = [project('p1')];
    const fetched = [project('p1', change)];

    expect(reuseUnchangedProjects(current, fetched)[0]).toBe(fetched[0]);
    // And the other way round: a field only the object shown carries.
    expect(reuseUnchangedProjects(fetched, current)[0]).toBe(current[0]);
  });

  it('follows the fetched order and membership, keeping the objects', async () => {
    const reuseUnchangedProjects = await reuse();
    const current = IDS.map((id) => project(id));
    const added = project('p5');
    const fetched = [added, ...freshRows().reverse().slice(0, 3)];

    const next = reuseUnchangedProjects(current, fetched);

    expect(next).not.toBe(current);
    expect(next.map((p) => p.id)).toEqual(['p5', 'p4', 'p3', 'p2']);
    expect(next[0]).toBe(added);
    expect(next[1]).toBe(current[3]);
    expect(next[2]).toBe(current[2]);
    expect(next[3]).toBe(current[1]);
  });

  it('follows a changed order of the same projects', async () => {
    const reuseUnchangedProjects = await reuse();
    const current = IDS.map((id) => project(id));

    const next = reuseUnchangedProjects(current, freshRows().reverse());

    expect(next).not.toBe(current);
    expect(next).toEqual([current[3], current[2], current[1], current[0]]);
    expect(next.every((p, i) => p === current[3 - i])).toBe(true);
  });

  it('is a new list when a project is only removed from the end', async () => {
    const reuseUnchangedProjects = await reuse();
    const current = IDS.map((id) => project(id));

    const next = reuseUnchangedProjects(current, freshRows().slice(0, 3));

    expect(next).not.toBe(current);
    expect(next).toEqual(current.slice(0, 3));
  });
});

describe('project-changed: identity', () => {
  it('keeps the object of every unchanged project and gives a changed one a new object', async () => {
    const { store } = await hydratedStore();
    const before = [...store.projects];
    rows[1] = project('p2', { name: 'renamed', updatedAt: 7 });

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.projects.map((p) => p.name)).toEqual(['p1', 'renamed', 'p3', 'p4']);
    expect(store.projects[0]).toBe(before[0]);
    expect(store.projects[1]).not.toBe(before[1]);
    expect(store.projects[1].updatedAt).toBe(7);
    expect(store.projects[2]).toBe(before[2]);
    expect(store.projects[3]).toBe(before[3]);
  });

  it('tells a field only the fetched row carries from the objects the store holds', async () => {
    const { store } = await hydratedStore();
    const before = [...store.projects];
    // The store's own objects are reactive proxies: a field one of them
    // lacks has to read as absent, not as equal.
    rows[1] = project('p2', { createdBy: 'session:s1' });

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.projects[1]).not.toBe(before[1]);
    expect(store.projects[1].createdBy).toBe('session:s1');
    expect(store.projects[0]).toBe(before[0]);

    // And once it carries the field, the same row again is unchanged.
    const withCreator = store.projects[1];
    const list = store.projects;
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(store.projects).toBe(list);
    expect(store.projects[1]).toBe(withCreator);

    // A row that drops the field again is a change.
    rows[1] = project('p2');
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(store.projects[1]).not.toBe(withCreator);
    expect(store.projects[1].createdBy).toBeUndefined();
  });

  it('does not apply a list that changes nothing', async () => {
    const { store } = await hydratedStore();
    // A marker makes liveProjects a list of its own, and gives the apply a
    // deleting map it could rewrite.
    store.projectDeleteStarted('p4', 'p4');
    const list = store.projects;
    const live = store.liveProjects;
    const watched = {
      projects: await watch(() => store.projects),
      liveProjects: await watch(() => store.liveProjects),
      branches: await watch(() => store.branchesByProject),
      repos: await watch(() => store.reposByProject),
      hydrated: await watch(() => store.isProjectHydrated('p1')),
      deleting: await watch(() => store.deletingProjectNames),
      repoCounts: await watch(() => store.repoCountsByProject),
    };

    // A write to a project this window does not list, and that the list
    // read after it does not hold either: created and deleted elsewhere.
    emit('project-changed', { projectId: 'gone' });
    await settle();

    expect(listProjects).toHaveBeenCalledTimes(1);
    expect(store.projects).toBe(list);
    expect(store.liveProjects).toBe(live);
    expect(Object.fromEntries(Object.entries(watched).map(([k, w]) => [k, w.wakes()]))).toEqual({
      projects: 0,
      liveProjects: 0,
      branches: 0,
      repos: 0,
      hydrated: 0,
      deleting: 0,
      repoCounts: 0,
    });
  });

  it('wakes the list once, and no map, for a change to one project’s row', async () => {
    const { store } = await hydratedStore();
    const watched = {
      projects: await watch(() => store.projects),
      branches: await watch(() => store.branchesByProject),
      repos: await watch(() => store.reposByProject),
      hydrated: await watch(() => store.isProjectHydrated('p1')),
    };
    rows[2] = project('p3', { name: 'renamed' });

    // The event names another project, one the list does not hold, so the
    // only change is p3's row: no project's lists are refetched.
    emit('project-changed', { projectId: 'gone' });
    await settle();

    expect(store.projects[2].name).toBe('renamed');
    expect(Object.fromEntries(Object.entries(watched).map(([k, w]) => [k, w.wakes()]))).toEqual({
      projects: 1,
      branches: 0,
      repos: 0,
      hydrated: 0,
    });
  });

  it('keeps the list and its maps through a hard reload that changes nothing', async () => {
    const { store, idle } = await hydratedStore();
    const before = [...store.projects];
    const watched = {
      projects: await watch(() => store.projects),
      branches: await watch(() => store.branchesByProject),
      repos: await watch(() => store.reposByProject),
    };

    emit('project-changed', { projectId: null });
    await settle();

    // The list read wakes nothing. The hydration that follows is what
    // replaces the lists, once for each map.
    expect(store.projects.every((p, i) => p === before[i])).toBe(true);
    expect(watched.projects.wakes()).toBe(0);
    expect(watched.branches.wakes()).toBe(0);
    expect(watched.repos.wakes()).toBe(0);
    idle.run();
    await settle();
    expect(watched.projects.wakes()).toBe(0);
    expect(watched.branches.wakes()).toBe(1);
    expect(watched.repos.wakes()).toBe(1);
  });

  it('keeps unchanged objects when refreshProject reads the list', async () => {
    const { store } = await hydratedStore();
    const list = store.projects;
    const before = [...list];

    expect(await store.refreshProject('p1')).toBe(true);
    expect(store.projects).toBe(list);

    rows[0] = project('p1', { githubRepo: 'org/other' });
    expect(await store.refreshProject('p1')).toBe(true);
    expect(store.projects).not.toBe(list);
    expect(store.projects[0]).not.toBe(before[0]);
    expect(store.projects[0].githubRepo).toBe('org/other');
    expect(store.projects.slice(1).every((p, i) => p === before[i + 1])).toBe(true);
  });

  it('seeds a new project and prunes a removed one without replacing the others', async () => {
    const { store } = await hydratedStore();
    const before = new Map(store.projects.map((p) => [p.id, p]));
    const p1Branches = store.branchesByProject.get('p1');
    const p1Repos = store.reposByProject.get('p1');
    rows = [project('p5'), project('p1'), project('p2'), project('p4')];

    emit('project-changed', { projectId: 'p5' });
    await settle();

    expect(projectIds(store)).toEqual(['p5', 'p1', 'p2', 'p4']);
    for (const id of ['p1', 'p2', 'p4']) {
      expect(store.projects.find((p) => p.id === id)).toBe(before.get(id));
    }
    // One entry per listed project, in list order: p5 seeded, p3 pruned.
    expect([...store.branchesByProject.keys()]).toEqual(['p5', 'p1', 'p2', 'p4']);
    expect(store.branchesByProject.get('p5')).toEqual([]);
    expect(store.branchesByProject.get('p1')).toBe(p1Branches);
    expect([...store.reposByProject.keys()]).toEqual(['p1', 'p2', 'p4']);
    expect(store.reposByProject.get('p1')).toBe(p1Repos);
    expect(store.isProjectHydrated('p3')).toBe(false);
    expect(store.isProjectHydrated('p1')).toBe(true);
  });
});

// ── Flows ──

describe('project-changed: delete', () => {
  it('prunes a project deleted in this window with its marker, and reads nothing of it', async () => {
    const { store } = await hydratedStore();
    const before = [...store.projects];
    store.projectDeleteStarted('p2', 'p2');
    expect(store.liveProjects.map((p) => p.id)).toEqual(['p1', 'p3', 'p4']);
    rows = rows.filter((row) => row.id !== 'p2');

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(projectIds(store)).toEqual(['p1', 'p3', 'p4']);
    expect(store.liveProjects.map((p) => p.id)).toEqual(['p1', 'p3', 'p4']);
    expect(store.isProjectDeleting('p2')).toBe(false);
    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(store.reposByProject.has('p2')).toBe(false);
    expect(store.isProjectHydrated('p2')).toBe(false);
    // The backend no longer has the project: a read of it would fail.
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    // The rest keep their objects and their lists.
    expect(store.projects[0]).toBe(before[0]);
    expect(store.projects[1]).toBe(before[2]);
    expect(store.projects[2]).toBe(before[3]);
    expect(branchNames(store, ['p1', 'p3', 'p4'])).toEqual({ p1: 'bulk', p3: 'bulk', p4: 'bulk' });
  });

  it('prunes a project deleted in another window, and reads nothing of it', async () => {
    const { store } = await hydratedStore();
    rows = rows.filter((row) => row.id !== 'p3');

    emit('project-changed', { projectId: 'p3' });
    await settle();

    expect(projectIds(store)).toEqual(['p1', 'p2', 'p4']);
    expect(store.branchesByProject.has('p3')).toBe(false);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
  });

  it('ignores an event for a project deleted before the store ever listed it', async () => {
    const { store } = await hydratedStore();
    const list = store.projects;

    emit('project-changed', { projectId: 'never-seen' });
    await settle();

    expect(store.projects).toBe(list);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(store.branchesByProject.has('never-seen')).toBe(false);
  });

  it('keeps a deleting project hidden through a reload that still lists it', async () => {
    const { store } = await hydratedStore();
    store.projectDeleteStarted('p2', 'p2');

    // Another write to the project lands while its teardown is running: the
    // row is still there, so the marker stays and the card stays hidden.
    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(projectIds(store)).toEqual(IDS);
    expect(store.isProjectDeleting('p2')).toBe(true);
    expect(store.liveProjects.map((p) => p.id)).toEqual(['p1', 'p3', 'p4']);
  });

  it('drops what lands for a deleted project after the list pruned it', async () => {
    const { store } = await hydratedStore();
    // A refresh of p2 sent before the delete: its list still holds p2.
    const staleList = deferred<SwrResult<Project[]>>();
    const staleBranches = deferred<SwrResult<Branch[]>>();
    const staleRepos = deferred<SwrResult<ProjectRepo[]>>();
    listProjects.mockReturnValueOnce(staleList.promise);
    listBranchesForProject.mockReturnValueOnce(staleBranches.promise);
    listProjectRepos.mockReturnValueOnce(staleRepos.promise);
    const refreshing = store.refreshProject('p2');
    const beforeDelete = freshRows();

    store.projectDeleteStarted('p2', 'p2');
    rows = rows.filter((row) => row.id !== 'p2');
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(projectIds(store)).toEqual(['p1', 'p3', 'p4']);

    // No new generation discards these: the list is refused as sent before
    // the one that landed, and the lists as those of an unlisted project.
    staleList.resolve(swr(beforeDelete));
    staleBranches.resolve(swr([branch('p2', { branchName: 'stale' })]));
    staleRepos.resolve(swr([repo('p2', { branchName: 'stale' })]));
    await refreshing;
    await settle();

    expect(projectIds(store)).toEqual(['p1', 'p3', 'p4']);
    expect(store.liveProjects.map((p) => p.id)).toEqual(['p1', 'p3', 'p4']);
    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(store.reposByProject.has('p2')).toBe(false);
    expect(store.isProjectHydrated('p2')).toBe(false);
    expect(store.isProjectDeleting('p2')).toBe(false);
  });

  it('does not bring a deleted project back when a bulk hydration covering it lands', async () => {
    const bulkBranches = deferred<SwrResult<Record<string, Branch[]>>>();
    const bulkRepos = deferred<SwrResult<Record<string, ProjectRepo[]>>>();
    listBranchesByProject.mockReturnValueOnce(bulkBranches.promise);
    listProjectReposByProject.mockReturnValueOnce(bulkRepos.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    rows = rows.filter((row) => row.id !== 'p2');
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(projectIds(store)).toEqual(['p1', 'p3', 'p4']);

    // Read before the delete, so it still lists p2.
    bulkBranches.resolve(swr(Object.fromEntries(IDS.map((id) => [id, [branch(id)]]))));
    bulkRepos.resolve(swr(Object.fromEntries(IDS.map((id) => [id, [repo(id)]]))));
    await sweep;
    await settle();

    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(store.reposByProject.has('p2')).toBe(false);
    expect(store.isProjectHydrated('p2')).toBe(false);
    expect(branchNames(store, ['p1', 'p3', 'p4'])).toEqual({ p1: 'bulk', p3: 'bulk', p4: 'bulk' });
    expect(store.allProjectsHydrated).toBe(true);
    // p2's hydration was in flight when it was deleted: once it settles
    // there is nothing left to refetch.
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
  });
});

describe('project-changed: create', () => {
  it('confirms a project created in this window, keeping the object it seeded', async () => {
    const { store } = await hydratedStore();
    const created = project('p5');
    store.projectCreated(created);
    await settle();
    const seeded = store.projects[0];
    expect(seeded.id).toBe('p5');
    expect(listBranchesForProject.mock.calls).toEqual([['p5']]);
    rows = [project('p5'), ...rows];

    // The repo attached to the new project lands with the event's refetch.
    listProjectRepos.mockResolvedValueOnce(
      swr([repo('p5', { branchName: 'attached' }), repo('p5', { id: 'p5-r2', isPrimary: false })])
    );
    emit('project-changed', { projectId: 'p5' });
    await settle();

    expect(projectIds(store)).toEqual(['p5', ...IDS]);
    expect(store.projects[0]).toBe(seeded);
    expect(listBranchesForProject.mock.calls).toEqual([['p5'], ['p5', FORCED]]);
    expect(store.reposByProject.get('p5')).toHaveLength(2);
    expect(store.isProjectHydrated('p5')).toBe(true);
  });

  it('registers a project the store has not seen, and leaves it to its first hydration', async () => {
    const { store, idle } = await hydratedStore();
    const before = [...store.projects];
    rows = [project('p5'), ...rows];

    emit('project-changed', { projectId: 'p5' });
    await settle();

    expect(projectIds(store)).toEqual(['p5', ...IDS]);
    expect(store.projects.slice(1).every((p, i) => p === before[i])).toBe(true);
    expect(store.branchesByProject.get('p5')).toEqual([]);
    expect(store.isProjectHydrated('p5')).toBe(false);
    // Not read twice: the hydration below is sent after the event, so it
    // reads what the write left.
    expect(listBranchesForProject).not.toHaveBeenCalled();

    idle.run();
    await settle();

    expect(listBranchesForProject.mock.calls).toEqual([['p5']]);
    expect(listProjectRepos.mock.calls).toEqual([['p5']]);
    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(branchNames(store, ['p5', 'p1'])).toEqual({ p5: 'single', p1: 'bulk' });
    expect(store.isProjectHydrated('p5')).toBe(true);
  });

  it('refetches a new project when a second event lands during its first hydration', async () => {
    const { store, idle } = await hydratedStore();
    rows = [project('p5'), ...rows];
    emit('project-changed', { projectId: 'p5' });
    await settle();

    // The first hydration goes out, and reads p5 before its repo is attached.
    const firstBranches = deferred<SwrResult<Branch[]>>();
    const firstRepos = deferred<SwrResult<ProjectRepo[]>>();
    listBranchesForProject.mockReturnValueOnce(firstBranches.promise);
    listProjectRepos.mockReturnValueOnce(firstRepos.promise);
    idle.run();
    await settle();
    expect(listBranchesForProject.mock.calls).toEqual([['p5']]);

    emit('project-changed', { projectId: 'p5' });
    await settle();
    // Held until that hydration settles, so the event's read lands last.
    expect(listBranchesForProject.mock.calls).toEqual([['p5']]);

    firstBranches.resolve(swr([]));
    firstRepos.resolve(swr([]));
    await settle();

    expect(listBranchesForProject.mock.calls).toEqual([['p5'], ['p5', FORCED]]);
    expect(listProjectRepos.mock.calls).toEqual([['p5'], ['p5', FORCED]]);
    expect(branchNames(store, ['p5'])).toEqual({ p5: 'single' });
    expect(repoBranchNames(store, ['p5'])).toEqual({ p5: 'single' });
    expect(store.isProjectHydrated('p5')).toBe(true);
  });
});

describe('project-changed: rename', () => {
  it('confirms a local rename and leaves every other project’s object alone', async () => {
    const { store } = await hydratedStore();
    const before = [...store.projects];
    store.projectRenamed('p2', 'Renamed');
    expect(store.projects[1].name).toBe('Renamed');
    rows[1] = project('p2', { name: 'Renamed', updatedAt: 9 });

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.projects[1]).toMatchObject({ name: 'Renamed', updatedAt: 9 });
    expect(store.projects[0]).toBe(before[0]);
    expect(store.projects[2]).toBe(before[2]);
    expect(store.projects[3]).toBe(before[3]);
  });

  it('does not put the old name back from a list read before the rename', async () => {
    const { store } = await hydratedStore();
    // A refresh of another project, sent before the rename.
    const staleList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(staleList.promise);
    const refreshing = store.refreshProject('p1');
    const beforeRename = freshRows();

    store.projectRenamed('p2', 'Renamed');
    rows[1] = project('p2', { name: 'Renamed', updatedAt: 9 });

    // It lands ahead of the rename's event. The soft path keeps the
    // generation, so nothing but the write's own mark turns it away.
    staleList.resolve(swr(beforeRename));
    await refreshing;
    expect(store.projects[1].name).toBe('Renamed');

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.projects[1]).toMatchObject({ name: 'Renamed', updatedAt: 9 });
  });
});

describe('project-changed: repo attach and detach', () => {
  it('shows an attached repo, asks for its badge, and keeps the other projects’ lists', async () => {
    const { store } = await hydratedStore();
    const p1Repos = store.reposByProject.get('p1');
    const p1Branches = store.branchesByProject.get('p1');
    listProjectRepos.mockResolvedValueOnce(
      swr([repo('p2'), repo('p2', { id: 'p2-r2', githubRepo: 'org/extra', isPrimary: false })])
    );
    listBranchesForProject.mockResolvedValueOnce(
      swr([branch('p2'), branch('p2', { id: 'p2-b2', projectRepoId: 'p2-r2' })])
    );

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.reposByProject.get('p2')?.map((r) => r.githubRepo)).toEqual([
      'org/p2',
      'org/extra',
    ]);
    expect(store.branchesByProject.get('p2')?.map((b) => b.id)).toEqual(['p2-b', 'p2-b2']);
    expect(store.repoCountsByProject.get('p2')).toBe(2);
    expect(ensureForRepos).toHaveBeenCalledWith([
      { githubRepo: 'org/p2', subpath: null },
      { githubRepo: 'org/extra', subpath: null },
    ]);
    expect(store.reposByProject.get('p1')).toBe(p1Repos);
    expect(store.branchesByProject.get('p1')).toBe(p1Branches);
  });

  it('shows a detached primary repo: the row, the repo list and the branch list', async () => {
    const { store } = await hydratedStore();
    const before = [...store.projects];
    // Removing the primary repo rewrites the project row as well.
    rows[1] = project('p2', { githubRepo: null, updatedAt: 3 });
    listProjectRepos.mockResolvedValueOnce(swr([]));
    listBranchesForProject.mockResolvedValueOnce(swr([]));

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.projects[1].githubRepo).toBeNull();
    expect(store.projects[1]).not.toBe(before[1]);
    expect(store.projects[0]).toBe(before[0]);
    expect(store.reposByProject.get('p2')).toEqual([]);
    expect(store.branchesByProject.get('p2')).toEqual([]);
    expect(store.repoCountsByProject.get('p2')).toBe(0);
  });
});

describe('project-changed: branch move', () => {
  it('refetches both sides of a move, the second named while the first reload is in flight', async () => {
    const { store } = await hydratedStore();
    const reload = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(reload.promise);
    const moved = branch('p1', { id: 'moved', projectId: 'p3', branchName: 'moved' });
    listBranchesForProject.mockImplementation(async (projectId: string) =>
      swr(projectId === 'p1' ? [] : [branch('p3'), moved])
    );

    // The backend publishes one event for each side.
    emit('project-changed', { projectId: 'p1' });
    emit('project-changed', { projectId: 'p3' });
    await settle();
    // One reload in flight, and the second event waits for the next.
    expect(listProjects).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();

    reload.resolve(swr(freshRows()));
    await settle();

    expect(listProjects.mock.calls).toEqual([[FORCED], [FORCED]]);
    expect(listBranchesForProject.mock.calls).toEqual([
      ['p1', FORCED],
      ['p3', FORCED],
    ]);
    expect(listProjectRepos.mock.calls).toEqual([
      ['p1', FORCED],
      ['p3', FORCED],
    ]);
    expect(store.branchesByProject.get('p1')).toEqual([]);
    expect(store.branchesByProject.get('p3')?.map((b) => b.id)).toEqual(['p3-b', 'moved']);
    expect(branchNames(store, ['p2', 'p4'])).toEqual({ p2: 'bulk', p4: 'bulk' });
  });

  it('names each project once when its events repeat behind a reload in flight', async () => {
    await hydratedStore();
    const reload = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(reload.promise);

    emit('project-changed', { projectId: 'p1' });
    emit('project-changed', { projectId: 'p3' });
    emit('project-changed', { projectId: 'p3' });
    emit('project-changed', { projectId: 'p1' });
    await settle();
    reload.resolve(swr(freshRows()));
    await settle();

    // p1 with the first reload, then p3 and p1 once each with the follow-up.
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listProjectRepos.mock.calls).toEqual([
      ['p1', FORCED],
      ['p3', FORCED],
      ['p1', FORCED],
    ]);

    // And they are not named again by the next follow-up.
    listProjectRepos.mockClear();
    const later = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(later.promise);
    emit('project-changed', { projectId: 'p4' });
    emit('project-changed', { projectId: 'p2' });
    await settle();
    later.resolve(swr(freshRows()));
    await settle();
    expect(listProjectRepos.mock.calls).toEqual([
      ['p4', FORCED],
      ['p2', FORCED],
    ]);
  });
});

describe('project-changed: transfer', () => {
  it('shows a transfer made elsewhere: the new location, and branches without the old worktrees', async () => {
    const { store } = await hydratedStore();
    expect(store.branchesByProject.get('p2')?.[0].worktreePath).toBe('/wt/p2');
    rows[1] = project('p2', { location: 'remote', updatedAt: 4 });
    listBranchesForProject.mockResolvedValueOnce(
      swr([branch('p2', { branchType: 'remote', worktreePath: null, workspaceName: 'ws' })])
    );

    emit('project-changed', { projectId: 'p2' });
    await settle();

    expect(store.projects[1].location).toBe('remote');
    expect(store.branchesByProject.get('p2')?.[0]).toMatchObject({
      branchType: 'remote',
      // Not carried over from the local branch the transfer removed.
      worktreePath: null,
    });
    expect(branchNames(store, ['p1', 'p3', 'p4'])).toEqual({ p1: 'bulk', p3: 'bulk', p4: 'bulk' });
  });

  it('lets the transferring window’s own refresh land without a retry', async () => {
    const { store } = await hydratedStore();
    rows[1] = project('p2', { location: 'remote', updatedAt: 4 });
    const remote = branch('p2', { branchType: 'remote', worktreePath: null, branchName: 'own' });
    const ownBranches = deferred<SwrResult<Branch[]>>();
    listBranchesForProject
      .mockReturnValueOnce(ownBranches.promise)
      .mockResolvedValue(swr([{ ...remote, branchName: 'event' }]));

    // The transfer settles, and its project-changed arrives while the
    // window's own refresh is in flight.
    const refreshing = store.refreshTransferredProject('p2');
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(branchNames(store).p2).toBe('event');
    ownBranches.resolve(swr([remote]));
    await refreshing;
    await settle();

    // Under the hard reload this refresh was overtaken and sent again. Now
    // it lands, and replaces the list outright: no old worktree path is left.
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listBranchesForProject).toHaveBeenCalledTimes(2);
    expect(store.projects[1].location).toBe('remote');
    expect(store.branchesByProject.get('p2')).toEqual([remote]);
  });
});

// ── Races ──

describe('project-changed: racing a load', () => {
  it('reloads hard when the first list has not landed, so every project is hydrated in bulk', async () => {
    const idle = holdIdleQueue();
    const first = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(first.promise);
    const store = await importStore();
    store.startListeners();
    const loading = store.ensureLoaded();
    await tick();

    emit('project-changed', { projectId: 'p4' });
    await settle();
    expect(store.loaded).toBe(true);
    expect(projectIds(store)).toEqual(IDS);

    // The first read was sent before the write: it lacks p4.
    first.resolve(swr(['p1', 'p2', 'p3'].map((id) => project(id))));
    await loading;
    await settle();
    expect(projectIds(store)).toEqual(IDS);

    idle.run();
    await settle();
    // A soft reload here would have found all four new and read each on its
    // own.
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('wins over a hard reload in flight whose list was read before the write', async () => {
    const { store } = await hydratedStore();
    const hardList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(hardList.promise);
    const refreshing = store.refresh();
    const beforeWrite = freshRows();
    await tick();

    rows[1] = project('p2', { name: 'renamed', updatedAt: 2 });
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(store.projects[1].name).toBe('renamed');
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);

    hardList.resolve(swr(beforeWrite));
    await refreshing;
    await settle();

    expect(store.projects[1].name).toBe('renamed');
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('refetches the named project after a hard reload it was queued behind', async () => {
    const { store, idle } = await hydratedStore();
    const hardList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(hardList.promise);

    emit('project-changed', { projectId: null });
    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(listProjects).toHaveBeenCalledTimes(1);

    hardList.resolve(swr(freshRows()));
    await settle();

    // The follow-up is soft, as only a soft caller queued it, and forced.
    expect(listProjects.mock.calls).toEqual([[FORCED], [FORCED]]);
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);
    expect(branchNames(store).p2).toBe('single');
    // The hard reload's own hydration is not cancelled by it.
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
  });

  it('reloads hard when an event that names no project queues behind a targeted reload', async () => {
    const { store, idle } = await hydratedStore();
    const softList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(softList.promise);

    emit('project-changed', { projectId: 'p2' });
    emit('project-changed', { projectId: 'p3' });
    emit('project-changed', { projectId: null });
    await settle();
    softList.resolve(swr(freshRows()));
    await settle();
    idle.run();
    await settle();

    // The follow-up is hard: it hydrates every project again, p3 included,
    // so p3 is not read on its own as well.
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);
    expect(store.allProjectsHydrated).toBe(true);
  });
});

describe('project-changed: racing a hydration', () => {
  it('keeps a bulk hydration in flight, and applies the named project’s refetch after it', async () => {
    const bulkBranches = deferred<SwrResult<Record<string, Branch[]>>>();
    const bulkRepos = deferred<SwrResult<Record<string, ProjectRepo[]>>>();
    listBranchesByProject.mockReturnValueOnce(bulkBranches.promise);
    listProjectReposByProject.mockReturnValueOnce(bulkRepos.promise);
    const { store } = await loadedStore();
    const sweep = store.ensureProjectsHydrated();
    await settle();

    emit('project-changed', { projectId: 'p2' });
    await settle();
    // Not read while its first hydration is out.
    expect(listBranchesForProject).not.toHaveBeenCalled();

    // The bulk replies were read before the write to p2.
    bulkBranches.resolve(swr(Object.fromEntries(IDS.map((id) => [id, [branch(id)]]))));
    bulkRepos.resolve(swr(Object.fromEntries(IDS.map((id) => [id, [repo(id)]]))));
    await sweep;
    await settle();

    // One bulk read, kept for the three projects the write did not touch.
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);
    expect(listProjectRepos.mock.calls).toEqual([['p2', FORCED]]);
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    expect(repoBranchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('refuses a read of the named project sent before the event that lands after its refetch', async () => {
    const { store } = await hydratedStore();
    // The selected project is hydrated again, and the read goes out before
    // the write.
    const staleBranches = deferred<SwrResult<Branch[]>>();
    const staleRepos = deferred<SwrResult<ProjectRepo[]>>();
    listBranchesForProject.mockReturnValueOnce(staleBranches.promise);
    listProjectRepos.mockReturnValueOnce(staleRepos.promise);
    const hydrating = store.hydrateProject('p2');
    await settle();

    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(branchNames(store).p2).toBe('single');

    // No new generation discards it: it is refused as sent before the list
    // that landed.
    staleBranches.resolve(swr([branch('p2', { branchName: 'stale' })]));
    staleRepos.resolve(swr([repo('p2', { branchName: 'stale' })]));
    await hydrating;
    await settle();

    expect(branchNames(store).p2).toBe('single');
    expect(repoBranchNames(store).p2).toBe('single');
  });

  it('refuses the named project’s entry in a bulk reply that lands after its refetch', async () => {
    const { store, idle } = await hydratedStore();
    // A hard reload's bulk hydration goes out, then p2 is written to.
    const bulkBranches = deferred<SwrResult<Record<string, Branch[]>>>();
    const bulkRepos = deferred<SwrResult<Record<string, ProjectRepo[]>>>();
    listBranchesByProject.mockReturnValueOnce(bulkBranches.promise);
    listProjectReposByProject.mockReturnValueOnce(bulkRepos.promise);
    emit('project-changed', { projectId: null });
    await settle();
    idle.run();
    await settle();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);

    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(branchNames(store).p2).toBe('single');

    const reloaded = (name: string) => ({ branchName: name });
    bulkBranches.resolve(
      swr(Object.fromEntries(IDS.map((id) => [id, [branch(id, reloaded('reloaded'))]])))
    );
    bulkRepos.resolve(
      swr(Object.fromEntries(IDS.map((id) => [id, [repo(id, reloaded('reloaded'))]])))
    );
    await settle();

    // p2 keeps the read sent after its write. The rest take the bulk reply.
    expect(branchNames(store)).toEqual({
      p1: 'reloaded',
      p2: 'single',
      p3: 'reloaded',
      p4: 'reloaded',
    });
    expect(repoBranchNames(store)).toEqual({
      p1: 'reloaded',
      p2: 'single',
      p3: 'reloaded',
      p4: 'reloaded',
    });
  });
});

describe('project-changed: racing a refresh', () => {
  it('hydrates a new project that a refresh of another project listed first', async () => {
    const { store, idle } = await hydratedStore();
    rows = [project('p5'), ...rows];
    // A refresh of p1 is sent after the create, and its list lands before
    // the create's event arrives. It shows p5 and arranges nothing for it.
    await store.refreshProject('p1');
    await settle();
    expect(projectIds(store)).toEqual(['p5', ...IDS]);
    expect(store.branchesByProject.has('p5')).toBe(false);
    for (const mock of READ_MOCKS()) mock.mockClear();

    emit('project-changed', { projectId: 'p5' });
    await settle();
    expect(store.branchesByProject.get('p5')).toEqual([]);
    idle.run();
    await settle();

    expect(listBranchesForProject.mock.calls).toEqual([['p5']]);
    expect(listProjectRepos.mock.calls).toEqual([['p5']]);
    expect(store.isProjectHydrated('p5')).toBe(true);
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('hydrates a new project when a later list lands before the event’s own', async () => {
    const { store, idle } = await hydratedStore();
    rows = [project('p5'), ...rows];
    const eventList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(eventList.promise);
    emit('project-changed', { projectId: 'p5' });
    await settle();

    // A refresh of p1 sent after the event's list lands before it.
    await store.refreshProject('p1');
    await settle();
    expect(projectIds(store)).toEqual(['p5', ...IDS]);

    // The event's list is refused as overtaken, and still seeds p5.
    eventList.resolve(swr(freshRows()));
    await settle();
    expect(store.branchesByProject.get('p5')).toEqual([]);
    idle.run();
    await settle();

    expect(listBranchesForProject.mock.calls).toEqual([['p1', FORCED], ['p5']]);
    expect(store.isProjectHydrated('p5')).toBe(true);
    expect(store.allProjectsHydrated).toBe(true);
  });

  it('refetches a project whose first fetch is a refresh sent before the write', async () => {
    // Loaded, with the idle hydration still waiting: nothing is fetched yet.
    const { store, idle } = await loadedStore();
    const staleBranches = deferred<SwrResult<Branch[]>>();
    const staleRepos = deferred<SwrResult<ProjectRepo[]>>();
    listBranchesForProject.mockReturnValueOnce(staleBranches.promise);
    listProjectRepos.mockReturnValueOnce(staleRepos.promise);
    const refreshing = store.refreshProject('p2');
    await settle();

    emit('project-changed', { projectId: 'p2' });
    await settle();
    // Held: p2 has nothing fetched to repair until the refresh lands.
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);

    staleBranches.resolve(swr([branch('p2', { branchName: 'stale' })]));
    staleRepos.resolve(swr([repo('p2', { branchName: 'stale' })]));
    await refreshing;
    await settle();

    expect(listBranchesForProject.mock.calls).toEqual([
      ['p2', FORCED],
      ['p2', FORCED],
    ]);
    expect(listProjectRepos.mock.calls).toEqual([
      ['p2', FORCED],
      ['p2', FORCED],
    ]);
    expect(branchNames(store).p2).toBe('single');
    expect(repoBranchNames(store).p2).toBe('single');

    // The idle hydration passes over p2, already fetched under this load,
    // so the refetch is what keeps the stale read from staying painted.
    idle.run();
    await settle();
    expect(branchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
    expect(repoBranchNames(store)).toEqual({ p1: 'bulk', p2: 'single', p3: 'bulk', p4: 'bulk' });
  });

  it('waits for an earlier refresh still out when a later one has settled', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { store } = await loadedStore();
    const staleBranches = deferred<SwrResult<Branch[]>>();
    listBranchesForProject
      .mockReturnValueOnce(staleBranches.promise)
      .mockRejectedValueOnce(new Error('offline'));
    const first = store.refreshProject('p2');
    expect(await store.refreshProject('p2')).toBe(false);
    expect(errors).toHaveBeenCalledTimes(1);

    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(listBranchesForProject).toHaveBeenCalledTimes(2);

    staleBranches.resolve(swr([branch('p2', { branchName: 'stale' })]));
    await first;
    await settle();

    expect(listBranchesForProject).toHaveBeenCalledTimes(3);
    expect(branchNames(store).p2).toBe('single');
  });

  it('does not refetch a project that was deleted while its refresh was out', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { store } = await loadedStore();
    const staleBranches = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(staleBranches.promise);
    const refreshing = store.refreshProject('p2');
    await settle();

    rows = rows.filter((row) => row.id !== 'p2');
    emit('project-changed', { projectId: 'p2' });
    await settle();
    staleBranches.resolve(swr([branch('p2', { branchName: 'stale' })]));
    await refreshing;
    await settle();

    expect(projectIds(store)).toEqual(['p1', 'p3', 'p4']);
    expect(listBranchesForProject).toHaveBeenCalledTimes(1);
    expect(store.branchesByProject.has('p2')).toBe(false);
    expect(errors).not.toHaveBeenCalled();
  });
});

describe('project-changed: a hydration still queued', () => {
  it('does not read a project deleted while it waited in the one-at-a-time queue', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    // Too few projects for a bulk hydration: they go one per idle step.
    rows = [project('p1'), project('p2')];
    listBranchesForProject.mockImplementation(async (projectId: string) => {
      if (!rows.some((row) => row.id === projectId)) throw new Error('Project not found');
      return swr([branch(projectId, { branchName: 'single' })]);
    });
    const { store, idle } = await loadedStore();

    rows = [project('p1')];
    emit('project-changed', { projectId: 'p2' });
    await settle();
    for (let step = 0; step < 3; step++) {
      idle.run();
      await settle();
    }

    expect(projectIds(store)).toEqual(['p1']);
    expect(listBranchesForProject.mock.calls).toEqual([['p1']]);
    expect(store.allProjectsHydrated).toBe(true);
    expect(errors).not.toHaveBeenCalled();
  });

  it('does not read a new project deleted before its first hydration went out', async () => {
    const { store, idle } = await hydratedStore();
    rows = [project('p5'), ...rows];
    emit('project-changed', { projectId: 'p5' });
    await settle();
    expect(projectIds(store)).toEqual(['p5', ...IDS]);

    rows = rows.slice(1);
    emit('project-changed', { projectId: 'p5' });
    await settle();
    idle.run();
    await settle();

    expect(projectIds(store)).toEqual(IDS);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listProjectRepos).not.toHaveBeenCalled();
  });

  it('does not refetch a project deleted during its one-project first hydration', async () => {
    const { store, idle } = await hydratedStore();
    rows = [project('p5'), ...rows];
    emit('project-changed', { projectId: 'p5' });
    await settle();
    const firstBranches = deferred<SwrResult<Branch[]>>();
    listBranchesForProject.mockReturnValueOnce(firstBranches.promise);
    idle.run();
    await settle();

    rows = rows.slice(1);
    emit('project-changed', { projectId: 'p5' });
    await settle();
    firstBranches.resolve(swr([branch('p5')]));
    await settle();

    expect(projectIds(store)).toEqual(IDS);
    expect(listBranchesForProject.mock.calls).toEqual([['p5']]);
    expect(store.branchesByProject.has('p5')).toBe(false);
    expect(store.isProjectHydrated('p5')).toBe(false);
  });
});

describe('project-changed: other reloads', () => {
  it('upgrades a view’s unforced reload in flight, and names the project to its follow-up', async () => {
    const { store } = await hydratedStore();
    const mounting = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(mounting.promise);
    void store.ensureLoaded();
    await settle();

    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(listProjects.mock.calls).toEqual([[{ force: false }]]);
    expect(listBranchesForProject).not.toHaveBeenCalled();

    mounting.resolve(swr(freshRows()));
    await settle();

    expect(listProjects.mock.calls).toEqual([[{ force: false }], [FORCED]]);
    expect(listBranchesForProject.mock.calls).toEqual([['p2', FORCED]]);
    expect(listProjectRepos.mock.calls).toEqual([['p2', FORCED]]);
    expect(listBranchesByProject).not.toHaveBeenCalled();
  });

  it('drops its list and its refetch when a full refresh starts after it', async () => {
    const { store } = await hydratedStore();
    const eventList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(eventList.promise);
    emit('project-changed', { projectId: 'p2' });
    await settle();

    const refreshing = store.refresh();
    await settle();
    eventList.resolve(swr([]));
    await refreshing;
    await settle();

    // The refresh was sent after the write and reads every project again.
    expect(projectIds(store)).toEqual(IDS);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
  });

  it('applies the refetched repos’ background revalidation', async () => {
    const { store } = await hydratedStore();
    const fresh = deferred<ProjectRepo[]>();
    listProjectRepos.mockResolvedValueOnce(
      swr([repo('p2', { branchName: 'cached' })], fresh.promise)
    );

    emit('project-changed', { projectId: 'p2' });
    await settle();
    expect(repoBranchNames(store).p2).toBe('cached');

    fresh.resolve([repo('p2', { branchName: 'fresh' })]);
    await settle();
    expect(repoBranchNames(store).p2).toBe('fresh');
  });

  it('prunes a deleted project on the re-read when a local rename set the event’s list aside', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    listBranchesForProject.mockImplementation(async (projectId: string) => {
      if (!rows.some((row) => row.id === projectId)) throw new Error('Project not found');
      return swr([branch(projectId, { branchName: 'single' })]);
    });
    const { store } = await hydratedStore();
    rows = rows.filter((row) => row.id !== 'p2');
    rows[0] = project('p1', { name: 'renamed', updatedAt: 2 });
    const eventList = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(eventList.promise);

    emit('project-changed', { projectId: 'p2' });
    await settle();
    store.projectRenamed('p1', 'renamed');
    eventList.resolve(swr(freshRows()));
    await settle();

    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(projectIds(store)).toEqual(['p1', 'p3', 'p4']);
    expect(store.projects[0].name).toBe('renamed');
    expect(store.branchesByProject.has('p2')).toBe(false);
    // The list that was set aside could not say p2 was gone, so p2 was read
    // once and the read failed.
    expect(errors).toHaveBeenCalledTimes(1);
  });
});

describe('project-changed: after a reload that failed', () => {
  it('reloads hard on the next event, even after a view’s reload has cleared the error', async () => {
    const { store, idle } = await hydratedStore();
    listProjects.mockRejectedValueOnce(new Error('offline'));
    emit('project-changed', { projectId: 'p1' });
    await settle();
    // p1's refetch went with the list that failed.
    expect(store.error).toBe('offline');
    expect(listBranchesForProject).not.toHaveBeenCalled();

    await store.ensureLoaded();
    await settle();
    expect(store.error).toBeNull();
    for (const mock of READ_MOCKS()) mock.mockClear();

    emit('project-changed', { projectId: 'p2' });
    await settle();
    idle.run();
    await settle();

    // Every project again, p1 included, and none on its own.
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    for (const mock of READ_MOCKS()) mock.mockClear();

    // Repaired: the event after that is targeted again.
    emit('project-changed', { projectId: 'p3' });
    await settle();
    idle.run();
    await settle();
    expect(listBranchesForProject.mock.calls).toEqual([['p3', FORCED]]);
    expect(listBranchesByProject).not.toHaveBeenCalled();
  });

  it('keeps a view’s reload soft when it was queued behind the reload that failed', async () => {
    const { store, idle } = await hydratedStore();
    const first = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(first.promise);
    void store.ensureLoaded();
    await settle();
    void store.ensureLoaded();
    await settle();

    first.reject(new Error('offline'));
    await settle();
    idle.run();
    await settle();

    // No event is behind it, so no project is read again.
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).not.toHaveBeenCalled();
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(store.error).toBeNull();
  });

  it('reloads hard for an event that was queued behind the reload that failed', async () => {
    const { store, idle } = await hydratedStore();
    const first = deferred<SwrResult<Project[]>>();
    listProjects.mockReturnValueOnce(first.promise);
    emit('project-changed', { projectId: 'p1' });
    await settle();
    emit('project-changed', { projectId: 'p2' });
    await settle();

    first.reject(new Error('offline'));
    await settle();
    idle.run();
    await settle();

    // A targeted follow-up would refetch p2 and leave p1 as it was.
    expect(listProjects).toHaveBeenCalledTimes(2);
    expect(listBranchesByProject).toHaveBeenCalledTimes(1);
    expect(listBranchesForProject).not.toHaveBeenCalled();
    expect(store.error).toBeNull();
  });
});
