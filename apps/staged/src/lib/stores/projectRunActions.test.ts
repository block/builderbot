import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RunningActionSnapshot, RunPhase } from '../features/actions/actions';

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

function runAction(
  branchId: string,
  executionId: string,
  phase: RunPhase | null = { type: 'running', endpoint: null }
): RunningActionSnapshot {
  return { executionId, actionType: 'run', branchId, phase } as RunningActionSnapshot;
}

/** Flush enough microtasks for the store's chained awaits to settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

/** Yield a macrotask, the way one project's branch response follows another. */
const nextTask = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

let invokeCommand: ReturnType<typeof vi.fn>;
/** One deferred per `get_all_running_actions` wave, in call order. */
let waves: Deferred<RunningActionSnapshot[]>[];

async function importStore() {
  const { projectRunActionsStore } = await import('./projectRunActions.svelte');
  return projectRunActionsStore;
}

/**
 * Hydrate and flush the microtask the bulk helper defers before invoking.
 * Wrapped in an object so the still-pending hydration isn't flattened into
 * the returned promise.
 */
async function hydrate(
  store: Awaited<ReturnType<typeof importStore>>,
  branches: Map<string, never>
): Promise<{ done: Promise<void> }> {
  const done = store.hydrateFromProjectBranches(branches);
  await settle();
  return { done };
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  waves = [];
  invokeCommand = vi.fn((command: string) => {
    if (command !== 'get_all_running_actions') throw new Error(`unexpected command ${command}`);
    const d = deferred<RunningActionSnapshot[]>();
    waves.push(d);
    return d.promise;
  });
  vi.doMock('../transport', () => ({ invokeCommand, isTauri: true }));
  vi.doMock('../features/actions/actions', () => ({
    listenToActionStatus: vi.fn(() => () => {}),
    listenToRunPhaseChanged: vi.fn(() => () => {}),
  }));
});

afterEach(() => {
  vi.doUnmock('../transport');
  vi.doUnmock('../features/actions/actions');
  vi.unstubAllGlobals();
});

describe('projectRunActionsStore.hydrateFromProjectBranches', () => {
  it('issues one snapshot request across overlapping calls', async () => {
    const store = await importStore();
    const branches = new Map([
      ['p1', [{ id: 'b1' }, { id: 'b2' }] as never],
      ['p2', [{ id: 'b3' }] as never],
    ]);

    const first = store.hydrateFromProjectBranches(branches);
    // Second call arrives while the first's request is still in flight.
    const second = store.hydrateFromProjectBranches(
      new Map([...branches, ['p3', [{ id: 'b4' }] as never]])
    );
    await settle();

    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(invokeCommand).toHaveBeenCalledWith('get_all_running_actions');

    waves[0].resolve([runAction('b1', 'e1')]);
    await Promise.all([first, second]);

    // Once settled, nothing is requested again.
    await hydrate(store, branches);
    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(store.getRunActionPhase('p1')).toBe('running');
    expect(store.getRunActionPhase('p2')).toBeNull();
    expect(store.getRunActionPhase('p3')).toBeNull();
  });

  it('hydrates 103 branches arriving project by project with a single invoke', async () => {
    const store = await importStore();
    // Startup shape: the data store reassigns branchesByProject once per
    // project response, each in its own task, growing the membership every
    // time. The snapshot resolves part-way through.
    const branches = new Map<string, never>();
    let total = 0;
    for (let p = 0; p < 50; p++) {
      const ids = p < 3 ? [`b${p}-0`, `b${p}-1`, `b${p}-2`] : [`b${p}-0`, `b${p}-1`];
      total += ids.length;
      branches.set(`p${p}`, ids.map((id) => ({ id })) as never);
      void store.hydrateFromProjectBranches(new Map(branches));
      if (p === 10) {
        await settle();
        waves[0].resolve([runAction('b7-1', 'e1'), runAction('b40-0', 'e2', { type: 'building' })]);
      }
      await nextTask();
    }
    expect(total).toBe(103);
    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(store.getRunActionPhase('p7')).toBe('running');
    // p40 loaded after the snapshot had been applied; its execution was in
    // the snapshot all along and surfaces now that the map knows the branch.
    expect(store.getRunActionPhase('p40')).toBe('building');
    expect(store.getRunActionPhase('p8')).toBeNull();
  });

  it('takes the phase from the snapshot and treats a null phase as building', async () => {
    const store = await importStore();
    const { done } = await hydrate(
      store,
      new Map([
        ['p1', [{ id: 'b1' }] as never],
        ['p2', [{ id: 'b2' }] as never],
        ['p3', [{ id: 'b3' }] as never],
      ])
    );
    waves[0].resolve([
      runAction('b1', 'e1', { type: 'building' }),
      runAction('b2', 'e2', null),
      // Non-run actions on a branch never surface as a run badge.
      { ...runAction('b3', 'e3'), actionType: 'test' } as RunningActionSnapshot,
    ]);
    await done;
    expect(store.getRunActionPhase('p1')).toBe('building');
    expect(store.getRunActionPhase('p2')).toBe('building');
    expect(store.getRunActionPhase('p3')).toBeNull();
    // The snapshot carried every phase inline: no per-execution follow-up.
    expect(invokeCommand).toHaveBeenCalledTimes(1);
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
    await settle();
    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(secondDone).toBe(false);

    waves[0].resolve([runAction('b1', 'e1')]);
    await second;
    expect(store.getRunActionPhase('p1')).toBe('running');
  });

  it('skips the call entirely when the project/branch membership is unchanged', async () => {
    const store = await importStore();
    const updateBranchProjectMap = vi.spyOn(store, 'updateBranchProjectMap');
    const { done: first } = await hydrate(
      store,
      new Map([
        ['p1', [{ id: 'b1', name: 'main' }] as never],
        ['p2', [{ id: 'b2' }] as never],
      ])
    );
    expect(updateBranchProjectMap).toHaveBeenCalledTimes(1);
    waves[0].resolve([runAction('b1', 'e1')]);
    await first;

    // The data store hands over a fresh Map (new Branch objects, different
    // project order) on every reassignment; same membership, so no work.
    await hydrate(
      store,
      new Map([
        ['p2', [{ id: 'b2' }] as never],
        ['p1', [{ id: 'b1', name: 'main*' }] as never],
      ])
    );
    expect(updateBranchProjectMap).toHaveBeenCalledTimes(1);
    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(store.getRunActionPhase('p1')).toBe('running');

    // A membership change re-enters and rebuilds the map; the snapshot has
    // already been applied, so no request is needed for the new branch.
    await hydrate(
      store,
      new Map([
        ['p1', [{ id: 'b1' }] as never],
        ['p2', [{ id: 'b2' }, { id: 'b3' }] as never],
      ])
    );
    expect(updateBranchProjectMap).toHaveBeenCalledTimes(2);
    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(store.getRunActionPhase('p2')).toBeNull();
  });

  it('retries after a rejected request', async () => {
    const store = await importStore();
    const branches = new Map([['p1', [{ id: 'b1' }] as never]]);

    const { done: first } = await hydrate(store, branches);
    waves[0].reject(new Error('backend down'));
    await first;
    expect(invokeCommand).toHaveBeenCalledTimes(1);

    // Same membership as the failed call: the rejection must have invalidated
    // the early-out so this one re-requests.
    const { done: retry } = await hydrate(store, branches);
    expect(invokeCommand).toHaveBeenCalledTimes(2);
    waves[1].resolve([runAction('b1', 'e1')]);
    await retry;
    expect(store.getRunActionPhase('p1')).toBe('running');

    // And a successful hydration is not retried.
    await hydrate(store, branches);
    expect(invokeCommand).toHaveBeenCalledTimes(2);
  });

  it('drops an in-flight answer after stopListening and re-requests next time', async () => {
    const store = await importStore();
    const branches = new Map([['p1', [{ id: 'b1' }] as never]]);

    const { done: stale } = await hydrate(store, branches);
    store.stopListening();
    // The stale answer arrives claiming an action is running; it must be
    // dropped rather than applied to the reset store.
    waves[0].resolve([runAction('b1', 'e-stale')]);
    await stale;

    const { done: fresh } = await hydrate(store, branches);
    expect(invokeCommand).toHaveBeenCalledTimes(2);
    waves[1].resolve([]);
    await fresh;
    // Had the stale answer been applied, the rebuilt branch→project map would
    // now surface its execution as running.
    expect(store.getRunActionPhase('p1')).toBeNull();
  });
});
