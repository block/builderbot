// Runs in the "component" vitest project for its svelte plugin, not for a
// DOM: the store's $state and $derived fields have to compile for the client,
// as in the app (see projectsData.component.test.ts).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SwrResult } from '../cache';
import type { Project, ProjectTag, Tag, TagState } from '../types';

// ── Fixtures ──

function tag(id: string, position: number): Tag {
  return {
    id,
    groupId: 'status',
    name: id,
    color: 'gray',
    icon: null,
    position,
    createdAt: 0,
    updatedAt: 0,
  };
}

function state(assignments: ProjectTag[] = []): TagState {
  return {
    groups: [
      {
        id: 'status',
        name: 'Status',
        position: 0,
        isIconGroup: true,
        showInSidebar: false,
        createdAt: 0,
        updatedAt: 0,
      },
    ],
    tags: [tag('idea', 0), tag('done', 1), tag('blocked', 2)],
    assignments,
  };
}

/** p1 tagged `tagId` in Status (or untagged). */
function withP1(tagId: string | null): TagState {
  return state(tagId ? [{ projectId: 'p1', tagId }] : []);
}

function project(id: string): Project {
  return {
    id,
    name: id,
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

// ── Mock plumbing ──

type EventCallback = (payload: unknown) => void;

/** Each `get_tag_state` call, in order, for the test to answer. */
let fetches: { force: boolean; answer: ReturnType<typeof deferred<SwrResult<TagState>>> }[];
let eventListeners: Map<string, EventCallback[]>;
let windowTarget: EventTarget;
let liveProjects: Project[];

/** Answer fetch `index` from the network. */
function answer(index: number, data: TagState): void {
  fetches[index].answer.resolve({ data, revalidating: null });
}

async function importStore() {
  const { tagsStore } = await import('./tags.svelte');
  return tagsStore;
}

beforeEach(() => {
  vi.resetModules();
  windowTarget = new EventTarget();
  vi.stubGlobal('window', windowTarget);
  fetches = [];
  eventListeners = new Map();
  liveProjects = [project('p1'), project('p2')];

  vi.doMock('../commands', () => ({
    getTagState: vi.fn(({ force = false }: { force?: boolean } = {}) => {
      const answer = deferred<SwrResult<TagState>>();
      fetches.push({ force, answer });
      return answer.promise;
    }),
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
  vi.doMock('./projectsData.svelte', () => ({
    projectsDataStore: {
      get liveProjects() {
        return liveProjects;
      },
    },
  }));
});

afterEach(() => {
  vi.doUnmock('../commands');
  vi.doUnmock('../transport');
  vi.doUnmock('./projectsData.svelte');
  vi.unstubAllGlobals();
});

// ── Tests ──

describe('loading', () => {
  it('stays unloaded until the first snapshot, then indexes it', async () => {
    const store = await importStore();
    const loading = store.load();
    expect(store.loaded).toBe(false);
    expect(store.iconTagFor('p1')).toBeNull();

    answer(0, withP1('done'));
    await loading;
    expect(store.loaded).toBe(true);
    expect(store.iconGroup?.id).toBe('status');
    expect(store.iconTagFor('p1')?.id).toBe('done');
    expect(store.tagsByGroup.get('status')!.map((t) => t.id)).toEqual(['idea', 'done', 'blocked']);
  });

  it('shares the startup load between callers', async () => {
    const store = await importStore();
    const first = store.load();
    const second = store.load();
    expect(fetches).toHaveLength(1);
    expect(fetches[0].force).toBe(false);
    answer(0, state());
    await Promise.all([first, second]);
  });

  it('settles as loaded on a failed first load, so icons fall back to the PR status', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const store = await importStore();
    const loading = store.load();
    fetches[0].answer.reject(new Error('backend down'));
    await loading;
    expect(store.loaded).toBe(true);
    expect(store.iconGroup).toBeNull();
  });

  it('counts usage over live projects only', async () => {
    const store = await importStore();
    const loading = store.load();
    answer(
      0,
      state([
        { projectId: 'p1', tagId: 'done' },
        { projectId: 'p2', tagId: 'done' },
        { projectId: 'gone', tagId: 'idea' },
      ])
    );
    await loading;
    expect(store.usageCount.get('done')).toBe(2);
    expect(store.usageCount.has('idea')).toBe(false);
  });
});

describe('freshness', () => {
  it('ignores a response older than one already applied', async () => {
    const store = await importStore();
    // The startup load and an echo's refresh are both out; the newer answers first.
    const older = store.load();
    const newer = store.requestRefresh();
    answer(1, withP1('done'));
    await newer;
    answer(0, withP1('idea'));
    await older;
    expect(store.iconTagFor('p1')?.id).toBe('done');
  });

  it('coalesces refreshes to one in flight and one queued, always forced', async () => {
    const store = await importStore();
    const first = store.requestRefresh();
    const second = store.requestRefresh();
    const third = store.requestRefresh();
    expect(second).toBe(third);
    expect(fetches).toHaveLength(1);

    answer(0, state());
    await first;
    await tick();
    expect(fetches).toHaveLength(2);
    answer(1, state());
    await second;
    expect(fetches.every((f) => f.force)).toBe(true);
  });

  it('discards responses already in flight when a confirmed write is patched in', async () => {
    const store = await importStore();
    const loading = store.load();
    answer(0, withP1(null));
    await loading;

    const inFlight = store.requestRefresh();
    store.patchLocal((s) => ({ ...s, tags: [...s.tags, tag('shipping', 3)] }));
    expect(store.tagsById.has('shipping')).toBe(true);

    // The in-flight response predates the write.
    answer(1, withP1(null));
    await inFlight;
    expect(store.tagsById.has('shipping')).toBe(true);

    // The refetch patchLocal queued was requested after it.
    await tick();
    expect(fetches).toHaveLength(3);
    answer(2, { ...withP1(null), tags: [...state().tags, tag('shipping', 3)] });
    await tick();
    expect(store.tagsById.has('shipping')).toBe(true);
  });

  it('applies a cached first value, then the network’s, without letting the cache clear intents', async () => {
    const store = await importStore();
    // A committed pick, whose refetch (fetch 0) is still out.
    const { slot, token } = store.beginIntent('p1', {
      kind: 'group',
      groupId: 'status',
      tagId: 'blocked',
    });
    store.confirmIntent('p1', slot, token);

    // The startup load (fetch 1) is requested after the commit, but its first
    // value comes from the web cache: shown, yet not proof of the write.
    const revalidated = deferred<TagState>();
    const loading = store.load();
    fetches[1].answer.resolve({ data: withP1('idea'), revalidating: revalidated.promise });
    await tick();
    expect(store.loaded).toBe(true);
    expect(store.tagsById.has('idea')).toBe(true);
    expect(store.iconTagFor('p1')?.id).toBe('blocked');

    // The network's answer for the same request clears it.
    revalidated.resolve(withP1('done'));
    await loading;
    expect(store.iconTagFor('p1')?.id).toBe('done');

    // And a late answer to the older request is ignored.
    answer(0, withP1('idea'));
    await tick();
    expect(store.iconTagFor('p1')?.id).toBe('done');
  });

  it('refetches on tags-changed and on cache-stale', async () => {
    const store = await importStore();
    store.startListeners();
    for (const callback of eventListeners.get('tags-changed') ?? []) callback({});
    expect(fetches).toHaveLength(1);
    answer(0, state());
    await tick();

    windowTarget.dispatchEvent(new Event('cache-stale'));
    expect(fetches).toHaveLength(2);
    expect(fetches.every((f) => f.force)).toBe(true);

    store.stopListeners();
    for (const callback of eventListeners.get('tags-changed') ?? []) callback({});
    windowTarget.dispatchEvent(new Event('cache-stale'));
    expect(fetches).toHaveLength(2);
  });
});

describe('pending writes', () => {
  async function loadedStore(initial: TagState) {
    const store = await importStore();
    const loading = store.load();
    answer(0, initial);
    await loading;
    return store;
  }

  it('shows an intent at once and keeps it over responses that predate its commit', async () => {
    const store = await loadedStore(withP1('idea'));
    // A refresh is already out (an echo of something else) when the user picks.
    const preCommit = store.requestRefresh();
    const { slot, token } = store.beginIntent('p1', {
      kind: 'group',
      groupId: 'status',
      tagId: 'done',
    });
    expect(store.iconTagFor('p1')?.id).toBe('done');

    // Still pending: a response doesn't clear it.
    answer(1, withP1('idea'));
    await preCommit;
    expect(store.iconTagFor('p1')?.id).toBe('done');

    // Committed: the confirm requests a snapshot that can include it...
    store.confirmIntent('p1', slot, token);
    await tick();
    expect(fetches).toHaveLength(3);
    // ...and only that one clears it.
    answer(2, withP1('done'));
    await tick();
    expect(store.iconTagFor('p1')?.id).toBe('done');
    // Server truth now, not the overlay: a later change shows through.
    const later = store.requestRefresh();
    answer(3, withP1('blocked'));
    await later;
    expect(store.iconTagFor('p1')?.id).toBe('blocked');
  });

  it('keeps a confirmed intent over a response requested before the confirm', async () => {
    const store = await loadedStore(withP1('idea'));
    const { slot, token } = store.beginIntent('p1', {
      kind: 'group',
      groupId: 'status',
      tagId: 'done',
    });
    // Requested before the write's command resolved: it may predate the commit.
    const beforeConfirm = store.requestRefresh();
    store.confirmIntent('p1', slot, token);
    answer(1, withP1('idea'));
    await beforeConfirm;
    expect(store.iconTagFor('p1')?.id).toBe('done');

    await tick();
    answer(2, withP1('done'));
    await tick();
    expect(store.iconTagFor('p1')?.id).toBe('done');
  });

  it('drops only the failed write’s own intent', async () => {
    const store = await loadedStore(withP1('idea'));
    const older = store.beginIntent('p1', { kind: 'group', groupId: 'status', tagId: 'done' });
    const newer = store.beginIntent('p1', { kind: 'group', groupId: 'status', tagId: 'blocked' });
    expect(older.slot).toBe(newer.slot);

    // The superseded write fails: the newer pick stays.
    store.dropIntent('p1', older.slot, older.token);
    expect(store.iconTagFor('p1')?.id).toBe('blocked');

    // The latest fails too: back to server truth.
    store.dropIntent('p1', newer.slot, newer.token);
    expect(store.iconTagFor('p1')?.id).toBe('idea');
  });

  it('keeps intents in other slots and projects independent', async () => {
    const store = await loadedStore(withP1('idea'));
    const status = store.beginIntent('p1', { kind: 'group', groupId: 'status', tagId: 'done' });
    store.beginIntent('p2', { kind: 'group', groupId: 'status', tagId: 'blocked' });
    store.dropIntent('p1', status.slot, status.token);
    expect(store.iconTagFor('p1')?.id).toBe('idea');
    expect(store.iconTagFor('p2')?.id).toBe('blocked');
  });
});
