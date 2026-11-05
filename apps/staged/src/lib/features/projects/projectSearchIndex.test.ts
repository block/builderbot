import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectSearchItem } from '../../types';

type Handler = (payload: unknown) => void;

let getProjectSearchIndex: ReturnType<typeof vi.fn>;
let handlers: Map<string, Handler[]>;
let unlistened: string[];
/** Each listener's onEstablished, to fire as the transport would. */
let established: Array<() => void>;

function item(overrides: Partial<ProjectSearchItem> = {}): ProjectSearchItem {
  return {
    projectId: 'p1',
    branchId: 'b1',
    kind: 'note',
    id: 'n1',
    title: 'Login Research',
    updatedAt: 1,
    ...overrides,
  };
}

function emit(event: string): void {
  for (const handler of handlers.get(event) ?? []) handler({});
}

/** A fresh store whose `searching` predicate the test controls. */
async function makeStore(searching: { value: boolean }) {
  const { ProjectSearchIndexStore } = await import('./projectSearchIndex.svelte');
  return new ProjectSearchIndexStore(() => searching.value);
}

async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  vi.stubGlobal('window', new EventTarget());
  handlers = new Map();
  unlistened = [];
  established = [];
  getProjectSearchIndex = vi.fn().mockResolvedValue([item()]);
  vi.doMock('../../commands', () => ({ getProjectSearchIndex }));
  vi.doMock('../../transport', () => ({
    listenToEvent: (event: string, handler: Handler, opts?: { onEstablished?: () => void }) => {
      handlers.set(event, [...(handlers.get(event) ?? []), handler]);
      if (opts?.onEstablished) established.push(opts.onEstablished);
      return () => unlistened.push(event);
    },
  }));
});

afterEach(() => {
  vi.doUnmock('../../commands');
  vi.doUnmock('../../transport');
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('groupSearchItems', () => {
  it('groups by project and lower-cases titles once', async () => {
    const { groupSearchItems } = await import('./projectSearchIndex.svelte');
    const index = groupSearchItems([
      item({ id: 'n1', projectId: 'p1', title: 'Login Research' }),
      item({ id: 'pn1', projectId: 'p2', kind: 'projectNote', branchId: null, title: 'Plan' }),
      item({ id: 'v1', projectId: 'p1', kind: 'review', title: 'Review Login' }),
    ]);
    expect([...index.keys()]).toEqual(['p1', 'p2']);
    expect(index.get('p1')).toEqual([
      { kind: 'note', id: 'n1', title: 'Login Research', text: 'login research', updatedAt: 1 },
      { kind: 'review', id: 'v1', title: 'Review Login', text: 'review login', updatedAt: 1 },
    ]);
    expect(index.get('p2')?.[0]).toMatchObject({ kind: 'projectNote', text: 'plan' });
  });
});

describe('ProjectSearchIndexStore', () => {
  it('loads nothing until asked, then once', async () => {
    const store = await makeStore({ value: false });
    expect(getProjectSearchIndex).not.toHaveBeenCalled();
    expect(store.index).toBeNull();

    store.ensureLoaded();
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);
    expect(store.index?.get('p1')?.[0]).toMatchObject({ text: 'login research' });

    // Fresh: a later focus or query start costs nothing.
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);
  });

  it('listens to every event a title can change or move on', async () => {
    const store = await makeStore({ value: false });
    store.ensureLoaded();
    expect([...handlers.keys()].sort()).toEqual([
      'branch-changed',
      'notes-changed',
      'project-changed',
      'review-changed',
    ]);
    store.stop();
    expect(unlistened).toHaveLength(4);
  });

  it('outside a search, a write waits for the next ensureLoaded', async () => {
    const store = await makeStore({ value: false });
    store.ensureLoaded();
    await flush();

    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(5_000);
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);
    // The stale index stays in place meanwhile.
    expect(store.index).not.toBeNull();

    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
  });

  it('during a search, a burst of writes refetches once after the delay', async () => {
    const searching = { value: true };
    const store = await makeStore(searching);
    store.ensureLoaded();
    await flush();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');

    getProjectSearchIndex.mockResolvedValue([item({ title: 'Renamed' })]);
    emit('notes-changed');
    emit('review-changed');
    emit('branch-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS - 1);
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
    expect(store.index?.get('p1')?.[0]).toMatchObject({ text: 'renamed' });
  });

  it('a search that ends before the delay runs leaves the refetch for later', async () => {
    const searching = { value: true };
    const store = await makeStore(searching);
    store.ensureLoaded();
    await flush();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');

    emit('notes-changed');
    searching.value = false;
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS + 1);
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);

    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
  });

  it('a write during an in-flight load reloads once it lands', async () => {
    const searching = { value: true };
    let resolveFirst!: (items: ProjectSearchItem[]) => void;
    getProjectSearchIndex.mockImplementationOnce(
      () => new Promise<ProjectSearchItem[]>((resolve) => (resolveFirst = resolve))
    );
    const store = await makeStore(searching);
    store.ensureLoaded();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');

    // The write lands mid-read, so the read may predate it: the delay
    // elapses while still in flight and the reload queues behind it.
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS + 1);
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);

    getProjectSearchIndex.mockResolvedValue([item({ title: 'Fresh' })]);
    resolveFirst([item({ title: 'Stale' })]);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
    expect(store.index?.get('p1')?.[0]).toMatchObject({ text: 'fresh' });
  });

  it('during a search, cache-stale refetches after the delay', async () => {
    const store = await makeStore({ value: true });
    store.ensureLoaded();
    await flush();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');

    // A page freeze or socket gap: change-feed events may be gone for good.
    getProjectSearchIndex.mockResolvedValue([item({ title: 'Renamed during the gap' })]);
    window.dispatchEvent(new Event('cache-stale'));
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
    expect(store.index?.get('p1')?.[0]).toMatchObject({ text: 'renamed during the gap' });
  });

  it('outside a search, cache-stale waits for the next ensureLoaded', async () => {
    const store = await makeStore({ value: false });
    store.ensureLoaded();
    await flush();

    window.dispatchEvent(new Event('cache-stale'));
    await vi.advanceTimersByTimeAsync(5_000);
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);

    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
  });

  it('stops following cache-stale once stopped', async () => {
    const store = await makeStore({ value: false });
    store.ensureLoaded();
    await flush();
    store.stop();

    window.dispatchEvent(new Event('cache-stale'));
    // Re-listens, but nothing marked it stale while stopped.
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);
  });

  it('a listener going live after the first read started marks it stale', async () => {
    const searching = { value: true };
    const store = await makeStore(searching);
    store.ensureLoaded();
    await flush();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');
    expect(established).toHaveLength(4);

    // Tauri registers asynchronously: a write between the read and the
    // listener going live reached neither. Every listener's establishment
    // coalesces into one refetch.
    for (const onEstablished of established) onEstablished();
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);

    // A web-socket reconnect re-establishes them all: one more read.
    for (const onEstablished of established) onEstablished();
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(3);
  });

  it('a failed load is retried on the next ensureLoaded, not in a loop', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    getProjectSearchIndex.mockRejectedValueOnce(new Error('boom'));
    const store = await makeStore({ value: true });
    store.ensureLoaded();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);
    expect(store.index).toBeNull();
    expect(error).toHaveBeenCalled();

    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
    expect(store.index).not.toBeNull();
  });

  it('keystrokes through a burst of writes wait for the coalesced refetch', async () => {
    const store = await makeStore({ value: true });
    store.ensureLoaded();
    await flush();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');

    // A running session publishes notes-changed while the user keeps typing:
    // each keystroke calls ensureLoaded, none of them reads on its own.
    for (let i = 0; i < 4; i++) {
      emit('notes-changed');
      store.ensureLoaded();
      await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS / 5);
    }
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);

    // Fresh again: further keystrokes cost nothing.
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
  });

  it('a failed reload retries on the next keystroke, even with a refetch pending', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const store = await makeStore({ value: true });
    store.ensureLoaded();
    await flush();
    const { REFETCH_DELAY_MS } = await import('./projectSearchIndex.svelte');

    getProjectSearchIndex.mockRejectedValueOnce(new Error('boom'));
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
    expect(error).toHaveBeenCalled();
    // The previous index keeps matching meanwhile.
    expect(store.index).not.toBeNull();

    // Another write schedules a refetch, but the failure doesn't wait on it.
    getProjectSearchIndex.mockResolvedValue([item({ title: 'Recovered' })]);
    emit('notes-changed');
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(3);
    expect(store.index?.get('p1')?.[0]).toMatchObject({ text: 'recovered' });

    // That read covered the write, so the timer finds nothing to do, and the
    // cleared failure means the next write waits for the timer again.
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(3);
    emit('notes-changed');
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(4);
  });

  it('a write while no search ran is read by the keystroke that starts one', async () => {
    const searching = { value: false };
    const store = await makeStore(searching);
    store.ensureLoaded();
    await flush();

    // No search, so no refetch is scheduled: only the next query can pick
    // the write up, and it does at once rather than waiting for another.
    emit('review-changed');
    searching.value = true;
    store.ensureLoaded();
    await flush();
    expect(getProjectSearchIndex).toHaveBeenCalledTimes(2);
  });
});
