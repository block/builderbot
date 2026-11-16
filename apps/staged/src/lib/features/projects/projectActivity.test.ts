import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectActivity } from '../../types';
import type { ProjectSort } from './projectSort';

type Handler = (payload: unknown) => void;

let getProjectActivity: ReturnType<typeof vi.fn>;
let handlers: Map<string, Handler[]>;
let unlistened: string[];
/** Each listener's onEstablished, to fire as the transport would. */
let established: Array<() => void>;
/** The window's sort, as the singleton's predicate reads it. */
let sortPrefs: { sort: ProjectSort };

function row(projectId: string, lastActivityAt: number): ProjectActivity {
  return { projectId, lastActivityAt };
}

function emit(event: string): void {
  for (const handler of handlers.get(event) ?? []) handler({});
}

function fire(type: string): void {
  window.dispatchEvent(new Event(type));
}

/** A fresh store whose `wanted` predicate the test controls. */
async function makeStore(wanted: { value: boolean } = { value: true }) {
  const { ProjectActivityStore } = await import('./projectActivity.svelte');
  return new ProjectActivityStore(() => wanted.value);
}

async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

/** A store with its first read landed: p1 last active at 10. */
async function loadedStore(wanted: { value: boolean } = { value: true }) {
  const store = await makeStore(wanted);
  store.ensureLoaded();
  await flush();
  return store;
}

/** Land a refetch with `rows`, as a write on the change feed would. */
async function refetch(rows: ProjectActivity[]): Promise<void> {
  const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');
  getProjectActivity.mockResolvedValue(rows);
  emit('notes-changed');
  // Up to the read landing, and no further: a release waiting on its own
  // timer must not run here.
  await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state (and its raw form) as identity.
  const state = (initial: unknown) => initial;
  vi.stubGlobal('$state', Object.assign(state, { raw: state }));
  vi.stubGlobal('window', new EventTarget());
  handlers = new Map();
  unlistened = [];
  established = [];
  sortPrefs = { sort: [] };
  getProjectActivity = vi.fn().mockResolvedValue([row('p1', 10)]);
  vi.doMock('../../commands', () => ({ getProjectActivity }));
  vi.doMock('../../transport', () => ({
    listenToEvent: (event: string, handler: Handler, opts?: { onEstablished?: () => void }) => {
      handlers.set(event, [...(handlers.get(event) ?? []), handler]);
      if (opts?.onEstablished) established.push(opts.onEstablished);
      return () => unlistened.push(event);
    },
  }));
  vi.doMock('./projectSortPrefs.svelte', () => ({ projectSortPrefs: sortPrefs }));
});

afterEach(() => {
  vi.doUnmock('../../commands');
  vi.doUnmock('../../transport');
  vi.doUnmock('./projectSortPrefs.svelte');
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('indexProjectActivity', () => {
  it('maps each project to its last activity', async () => {
    const { indexProjectActivity } = await import('./projectActivity.svelte');
    const activity = indexProjectActivity([row('p1', 10), row('p2', 25)]);
    expect([...activity]).toEqual([
      ['p1', 10],
      ['p2', 25],
    ]);
    expect(indexProjectActivity([]).size).toBe(0);
  });
});

describe('ProjectActivityStore loading', () => {
  it('loads nothing until asked, then once', async () => {
    const store = await makeStore();
    expect(getProjectActivity).not.toHaveBeenCalled();
    expect(store.activity).toBeNull();
    // Nor does it listen for anything, the pointer included.
    expect(handlers.size).toBe(0);

    store.ensureLoaded();
    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(1);
    expect(store.activity?.get('p1')).toBe(10);

    // Fresh: asking again costs nothing.
    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(1);
  });

  it('listens to every event a project’s activity can move on', async () => {
    const store = await makeStore();
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

  it.each([['notes-changed'], ['review-changed'], ['branch-changed'], ['project-changed']])(
    'while the sort uses it, %s refetches after the delay',
    async (event) => {
      const store = await loadedStore();
      const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');

      getProjectActivity.mockResolvedValue([row('p1', 99)]);
      emit(event);
      await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS - 1);
      expect(getProjectActivity).toHaveBeenCalledTimes(1);

      await vi.advanceTimersByTimeAsync(1);
      expect(getProjectActivity).toHaveBeenCalledTimes(2);
      expect(store.activity?.get('p1')).toBe(99);
    }
  );

  it('a burst of writes refetches once', async () => {
    const store = await loadedStore();
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');

    getProjectActivity.mockResolvedValue([row('p1', 99)]);
    // A session finishing: its note, its commit, its branch.
    emit('notes-changed');
    emit('branch-changed');
    emit('review-changed');
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS * 4);

    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(99);
  });

  it('while the sort does not use it, a write waits for the next ensureLoaded', async () => {
    const wanted = { value: false };
    const store = await loadedStore(wanted);

    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);
    // The stale map stays in place meanwhile.
    expect(store.activity).not.toBeNull();

    // The sort gains a last-updated key again: read at once.
    wanted.value = true;
    getProjectActivity.mockResolvedValue([row('p1', 99)]);
    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(99);
  });

  it('a sort that drops its key before the delay runs leaves the refetch for later', async () => {
    const wanted = { value: true };
    const store = await loadedStore(wanted);
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');

    emit('notes-changed');
    wanted.value = false;
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS + 1);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    wanted.value = true;
    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
  });

  it('ensureLoaded leaves a write to its coalesced refetch', async () => {
    const store = await loadedStore();
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');

    emit('notes-changed');
    store.ensureLoaded();
    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
  });

  it('a write during an in-flight load reloads once it lands', async () => {
    let resolveFirst!: (rows: ProjectActivity[]) => void;
    getProjectActivity.mockImplementationOnce(
      () => new Promise<ProjectActivity[]>((resolve) => (resolveFirst = resolve))
    );
    const store = await makeStore();
    store.ensureLoaded();
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');

    // The write lands mid-read, so the read may predate it: the delay
    // elapses while still in flight and the reload queues behind it.
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS + 1);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    getProjectActivity.mockResolvedValue([row('p1', 99)]);
    resolveFirst([row('p1', 10)]);
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(99);
  });

  it('cache-stale refetches after the delay', async () => {
    const store = await loadedStore();
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');

    // A page freeze or socket gap: change-feed events may be gone for good.
    getProjectActivity.mockResolvedValue([row('p1', 99)]);
    fire('cache-stale');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(99);
  });

  it('a listener going live after the first read started marks it stale', async () => {
    await loadedStore();
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');
    expect(established).toHaveLength(4);

    // Tauri registers asynchronously: a write between the read and the
    // listener going live reached neither. Every listener's establishment
    // coalesces into one refetch.
    for (const onEstablished of established) onEstablished();
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);

    // A web-socket reconnect re-establishes them all: one more read.
    for (const onEstablished of established) onEstablished();
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(3);
  });

  it('stops following the feed, the window and the pointer once stopped', async () => {
    const store = await loadedStore();
    const { REFETCH_DELAY_MS } = await import('./projectActivity.svelte');
    emit('notes-changed');
    store.stop();

    // The refetch that was waiting is gone, and nothing schedules another.
    fire('cache-stale');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS * 4);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);
  });
});

describe('ProjectActivityStore after a failed load', () => {
  it('retries, backing off, and keeps the last map meanwhile', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const store = await loadedStore();
    const { REFETCH_DELAY_MS, RETRY_DELAY_MS } = await import('./projectActivity.svelte');
    const before = store.activity;

    getProjectActivity.mockRejectedValue(new Error('boom'));
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(error).toHaveBeenCalled();
    expect(store.activity).toBe(before);

    // First retry after RETRY_DELAY_MS, the next after twice that.
    await vi.advanceTimersByTimeAsync(RETRY_DELAY_MS - 1);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(getProjectActivity).toHaveBeenCalledTimes(3);

    await vi.advanceTimersByTimeAsync(RETRY_DELAY_MS * 2 - 1);
    expect(getProjectActivity).toHaveBeenCalledTimes(3);
    getProjectActivity.mockResolvedValue([row('p1', 99)]);
    await vi.advanceTimersByTimeAsync(1);
    expect(getProjectActivity).toHaveBeenCalledTimes(4);
    expect(store.activity?.get('p1')).toBe(99);

    // Recovered: nothing more is read until the next write.
    await vi.advanceTimersByTimeAsync(600_000);
    expect(getProjectActivity).toHaveBeenCalledTimes(4);
  });

  it('retries a failed first load without being asked again', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    getProjectActivity.mockRejectedValueOnce(new Error('boom'));
    const store = await makeStore();
    const { RETRY_DELAY_MS } = await import('./projectActivity.svelte');

    store.ensureLoaded();
    await flush();
    expect(store.activity).toBeNull();

    await vi.advanceTimersByTimeAsync(RETRY_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(10);
  });

  it('never waits longer than the cap between retries', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    getProjectActivity.mockRejectedValue(new Error('boom'));
    const store = await makeStore();
    const { MAX_RETRY_DELAY_MS } = await import('./projectActivity.svelte');
    store.ensureLoaded();
    // Long enough for the doubling to reach the cap.
    await vi.advanceTimersByTimeAsync(MAX_RETRY_DELAY_MS * 4);
    const calls = getProjectActivity.mock.calls.length;

    await vi.advanceTimersByTimeAsync(MAX_RETRY_DELAY_MS);

    expect(getProjectActivity.mock.calls.length).toBe(calls + 1);
  });

  it('starts the backoff over after a load lands', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    await loadedStore();
    const { REFETCH_DELAY_MS, RETRY_DELAY_MS } = await import('./projectActivity.svelte');

    // Two failures, then a recovery.
    getProjectActivity.mockRejectedValue(new Error('boom'));
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS + RETRY_DELAY_MS);
    getProjectActivity.mockResolvedValue([row('p1', 20)]);
    await vi.advanceTimersByTimeAsync(RETRY_DELAY_MS * 2);
    expect(getProjectActivity).toHaveBeenCalledTimes(4);

    // The next failure waits the first delay again, not four times it.
    getProjectActivity.mockRejectedValueOnce(new Error('boom'));
    getProjectActivity.mockResolvedValue([row('p1', 30)]);
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS + RETRY_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(6);
  });

  it('does not retry while the sort does not use it', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const wanted = { value: true };
    getProjectActivity.mockRejectedValueOnce(new Error('boom'));
    const store = await makeStore(wanted);
    store.ensureLoaded();
    wanted.value = false;
    await vi.advanceTimersByTimeAsync(600_000);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    // Asked again once a last-updated key is back: read at once.
    wanted.value = true;
    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(10);
  });

  it('with no map yet, ensureLoaded does not wait out the retry', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    getProjectActivity.mockRejectedValueOnce(new Error('boom'));
    const store = await makeStore();
    store.ensureLoaded();
    await flush();

    store.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity?.get('p1')).toBe(10);

    // That read covered the retry, which doesn't run on top of it.
    await vi.advanceTimersByTimeAsync(600_000);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
  });
});

describe('ProjectActivityStore map identity', () => {
  it('keeps the same map when a refetch changes nothing', async () => {
    const store = await loadedStore();
    const before = store.activity;

    await refetch([row('p1', 10)]);

    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(store.activity).toBe(before);
  });

  it.each<[string, ProjectActivity[]]>([
    ['a timestamp moved', [row('p1', 11)]],
    ['a project gained activity', [row('p1', 10), row('p2', 5)]],
    ['a project lost its activity', []],
    ['one project swapped for another', [row('p2', 10)]],
  ])('hands out a new map when %s', async (_what, rows) => {
    const store = await loadedStore();
    const before = store.activity;

    await refetch(rows);

    expect(store.activity).not.toBe(before);
    expect([...store.activity!]).toEqual(rows.map((r) => [r.projectId, r.lastActivityAt]));
  });
});

describe('ProjectActivityStore under the pointer', () => {
  it('holds a new map during a press and applies it once the press ends', async () => {
    const store = await loadedStore();
    const before = store.activity;

    fire('pointerdown');
    await refetch([row('p1', 99)]);
    expect(store.activity).toBe(before);

    // Not in the releasing event itself: the click it makes comes after,
    // and must still find its row where it was pressed.
    fire('pointerup');
    expect(store.activity).toBe(before);
    fire('click');
    expect(store.activity).toBe(before);

    await flush();
    expect(store.activity?.get('p1')).toBe(99);
  });

  it.each([['pointerup'], ['pointercancel'], ['click'], ['blur']])(
    '%s ends a press',
    async (type) => {
      const store = await loadedStore();
      fire('pointerdown');
      await refetch([row('p1', 99)]);

      fire(type);
      await flush();

      expect(store.activity?.get('p1')).toBe(99);
    }
  );

  it('applies a map at once with no pointer down', async () => {
    const store = await loadedStore();
    fire('pointerdown');
    fire('pointerup');
    await flush();

    await refetch([row('p1', 99)]);

    expect(store.activity?.get('p1')).toBe(99);
  });

  it('holds the first map too', async () => {
    let resolveFirst!: (rows: ProjectActivity[]) => void;
    getProjectActivity.mockImplementationOnce(
      () => new Promise<ProjectActivity[]>((resolve) => (resolveFirst = resolve))
    );
    const store = await makeStore();
    store.ensureLoaded();

    fire('pointerdown');
    resolveFirst([row('p1', 10)]);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.activity).toBeNull();

    fire('pointerup');
    await flush();
    expect(store.activity?.get('p1')).toBe(10);
  });

  it('holds a new map through a card drag and applies it after the drop', async () => {
    const store = await loadedStore();
    const before = store.activity;

    fire('pointerdown');
    fire('dragstart');
    // A browser cancels the pointer as the drag takes over: the drag holds.
    fire('pointercancel');
    await vi.advanceTimersByTimeAsync(0);
    await refetch([row('p1', 99)]);
    expect(store.activity).toBe(before);

    fire('drop');
    expect(store.activity).toBe(before);
    await flush();
    expect(store.activity?.get('p1')).toBe(99);
  });

  it('applies it after a drag that lands nowhere', async () => {
    const store = await loadedStore();
    fire('pointerdown');
    fire('dragstart');
    await refetch([row('p1', 99)]);

    fire('dragend');
    await flush();

    expect(store.activity?.get('p1')).toBe(99);
  });

  it('a drag whose end was never seen stops holding at the next press', async () => {
    const store = await loadedStore();
    const before = store.activity;
    fire('pointerdown');
    fire('dragstart');
    await refetch([row('p1', 99)]);
    // The card left the page mid-drag: no dragend, no drop.
    expect(store.activity).toBe(before);

    fire('pointerdown');
    fire('pointerup');
    await flush();

    expect(store.activity?.get('p1')).toBe(99);
  });

  it('keeps holding when another press starts before the release runs', async () => {
    const store = await loadedStore();
    const before = store.activity;
    fire('pointerdown');
    await refetch([row('p1', 99)]);

    fire('pointerup');
    fire('pointerdown');
    await flush();
    expect(store.activity).toBe(before);

    fire('pointerup');
    await flush();
    expect(store.activity?.get('p1')).toBe(99);
  });

  it('applies only the latest of the maps that landed during a press', async () => {
    const store = await loadedStore();
    fire('pointerdown');
    await refetch([row('p1', 50)]);
    await refetch([row('p1', 99)]);

    fire('pointerup');
    await flush();

    expect(store.activity?.get('p1')).toBe(99);
  });

  it('drops a held map when a later read is back where the list already is', async () => {
    const store = await loadedStore();
    const before = store.activity;
    fire('pointerdown');
    await refetch([row('p1', 99)]);
    // A note written and deleted again within the press.
    await refetch([row('p1', 10)]);

    fire('pointerup');
    await flush();

    expect(store.activity).toBe(before);
  });
});

describe('projectActivity, the window’s store', () => {
  it('refetches on a write only while the sort has a last-updated key', async () => {
    const { projectActivity, REFETCH_DELAY_MS } = await import('./projectActivity.svelte');
    sortPrefs.sort = [{ kind: 'group', groupId: 'priority', direction: 'asc' }];
    projectActivity.ensureLoaded();
    await flush();
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS * 2);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    sortPrefs.sort = [
      { kind: 'group', groupId: 'priority', direction: 'asc' },
      { kind: 'updated', direction: 'desc' },
    ];
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    projectActivity.stop();
  });
});
