import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RunningActionInfo } from '../features/actions/actions';

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function runAction(branchId: string, executionId: string): RunningActionInfo {
  return { executionId, actionType: 'run', branchId } as RunningActionInfo;
}

/** Flush enough microtasks for the store's chained awaits to settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

let getRunningBranchActions: ReturnType<typeof vi.fn>;
let getRunPhase: ReturnType<typeof vi.fn>;
let requests: Map<string, Deferred<RunningActionInfo[]>[]>;

function requestsFor(branchId: string): Deferred<RunningActionInfo[]>[] {
  return requests.get(branchId) ?? [];
}

async function importStore() {
  const { projectRunActionsStore } = await import('./projectRunActions.svelte');
  return projectRunActionsStore;
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  requests = new Map();
  getRunningBranchActions = vi.fn((branchId: string) => {
    const d = deferred<RunningActionInfo[]>();
    requests.set(branchId, [...requestsFor(branchId), d]);
    return d.promise;
  });
  getRunPhase = vi.fn().mockResolvedValue({ type: 'running' });
  vi.doMock('../features/actions/actions', () => ({
    getRunningBranchActions,
    getRunPhase,
    listenToActionStatus: vi.fn(() => () => {}),
    listenToRunPhaseChanged: vi.fn(() => () => {}),
  }));
});

afterEach(() => {
  vi.doUnmock('../features/actions/actions');
  vi.unstubAllGlobals();
});

describe('projectRunActionsStore.hydrateFromProjectBranches', () => {
  it('issues one request per branch across overlapping calls', async () => {
    const store = await importStore();
    const branches = new Map([
      ['p1', [{ id: 'b1' }, { id: 'b2' }] as never],
      ['p2', [{ id: 'b3' }] as never],
    ]);

    const first = store.hydrateFromProjectBranches(branches);
    // Second call arrives while every request from the first is still in flight.
    const second = store.hydrateFromProjectBranches(
      new Map([...branches, ['p3', [{ id: 'b4' }] as never]])
    );

    expect(getRunningBranchActions).toHaveBeenCalledTimes(4);
    for (const id of ['b1', 'b2', 'b3', 'b4']) expect(requestsFor(id)).toHaveLength(1);

    requestsFor('b1')[0].resolve([runAction('b1', 'e1')]);
    for (const id of ['b2', 'b3', 'b4']) requestsFor(id)[0].resolve([]);
    await Promise.all([first, second]);

    // Once settled, the same branches are not requested again.
    await store.hydrateFromProjectBranches(branches);
    expect(getRunningBranchActions).toHaveBeenCalledTimes(4);
    expect(store.getRunActionPhase('p1')).toBe('running');
    expect(store.getRunActionPhase('p2')).toBeNull();
  });

  it('awaits the in-flight request instead of resolving early for a concurrent caller', async () => {
    const store = await importStore();
    const branches = new Map([['p1', [{ id: 'b1' }] as never]]);

    void store.hydrateFromProjectBranches(branches);
    let secondDone = false;
    const second = store
      .hydrateFromProjectBranches(new Map([...branches, ['p2', [{ id: 'b2' }] as never]]))
      .then(() => {
        secondDone = true;
      });
    expect(getRunningBranchActions).toHaveBeenCalledTimes(2);
    requestsFor('b2')[0].resolve([]);
    await settle();
    expect(secondDone).toBe(false);

    requestsFor('b1')[0].resolve([runAction('b1', 'e1')]);
    await second;
    expect(store.getRunActionPhase('p1')).toBe('running');
  });

  it('skips the call entirely when the project/branch membership is unchanged', async () => {
    const store = await importStore();
    const updateBranchProjectMap = vi.spyOn(store, 'updateBranchProjectMap');
    const first = store.hydrateFromProjectBranches(
      new Map([
        ['p1', [{ id: 'b1', name: 'main' }] as never],
        ['p2', [{ id: 'b2' }] as never],
      ])
    );
    expect(updateBranchProjectMap).toHaveBeenCalledTimes(1);
    requestsFor('b1')[0].resolve([runAction('b1', 'e1')]);
    requestsFor('b2')[0].resolve([]);
    await first;

    // The data store hands over a fresh Map (new Branch objects, different
    // project order) on every reassignment; same membership, so no work.
    await store.hydrateFromProjectBranches(
      new Map([
        ['p2', [{ id: 'b2' }] as never],
        ['p1', [{ id: 'b1', name: 'main*' }] as never],
      ])
    );
    expect(updateBranchProjectMap).toHaveBeenCalledTimes(1);
    expect(getRunningBranchActions).toHaveBeenCalledTimes(2);
    expect(store.getRunActionPhase('p1')).toBe('running');

    // A membership change re-enters: the map is rebuilt and only the new
    // branch is requested.
    const grown = store.hydrateFromProjectBranches(
      new Map([
        ['p1', [{ id: 'b1' }] as never],
        ['p2', [{ id: 'b2' }, { id: 'b3' }] as never],
      ])
    );
    expect(updateBranchProjectMap).toHaveBeenCalledTimes(2);
    expect(getRunningBranchActions).toHaveBeenCalledTimes(3);
    requestsFor('b3')[0].resolve([runAction('b3', 'e3')]);
    await grown;
    expect(store.getRunActionPhase('p2')).toBe('running');
  });

  it('retries a branch whose request was rejected', async () => {
    const store = await importStore();
    const branches = new Map([['p1', [{ id: 'b1' }] as never]]);

    const first = store.hydrateFromProjectBranches(branches);
    requestsFor('b1')[0].reject(new Error('backend down'));
    await first;
    expect(getRunningBranchActions).toHaveBeenCalledTimes(1);

    // Same membership as the failed call: the rejection must have invalidated
    // the early-out so this one re-requests the branch.
    const retry = store.hydrateFromProjectBranches(branches);
    expect(getRunningBranchActions).toHaveBeenCalledTimes(2);
    requestsFor('b1')[1].resolve([runAction('b1', 'e1')]);
    await retry;
    expect(store.getRunActionPhase('p1')).toBe('running');

    // And a successful hydration is not retried.
    await store.hydrateFromProjectBranches(branches);
    expect(getRunningBranchActions).toHaveBeenCalledTimes(2);
  });

  it('drops an in-flight answer after stopListening and re-requests the branch next time', async () => {
    const store = await importStore();
    const branches = new Map([['p1', [{ id: 'b1' }] as never]]);

    const stale = store.hydrateFromProjectBranches(branches);
    store.stopListening();
    // The stale answer arrives claiming an action is running; it must be
    // dropped rather than applied to the reset store.
    requestsFor('b1')[0].resolve([runAction('b1', 'e-stale')]);
    await stale;

    const fresh = store.hydrateFromProjectBranches(branches);
    expect(getRunningBranchActions).toHaveBeenCalledTimes(2);
    requestsFor('b1')[1].resolve([]);
    await fresh;
    // Had the stale answer been applied, the rebuilt branch→project map would
    // now surface its execution as running.
    expect(store.getRunActionPhase('p1')).toBeNull();
  });
});
