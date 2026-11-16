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

type Store = Awaited<ReturnType<typeof importStore>>;

/**
 * Count how often the readers of `read()` are woken, the way a card's
 * $derived or a template re-runs. Imported after the store, so both share
 * the module graph (and with it the Svelte runtime) that beforeEach's
 * resetModules started. `wakes()` runs pending effects and returns the
 * number of runs since the last call; the first run, on subscribing, is not
 * counted.
 */
async function watch(
  read: () => unknown,
  { derived = false }: { derived?: boolean } = {}
): Promise<{ wakes: () => number }> {
  const { readReactively, readDerivedReactively } =
    await import('./testStubs/readReactively.svelte');
  const { flushSync } = await import('svelte');
  let runs = 0;
  if (derived) {
    readDerivedReactively(read, () => runs++);
  } else {
    readReactively(() => {
      read();
      runs++;
    });
  }
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

/**
 * The same count for what reads a card's `$derived(read())`: the icon or
 * chip under it. The derived re-runs with the store, and its readers are
 * woken only when it comes out with a different object.
 */
function watchDerived(read: () => unknown): Promise<{ wakes: () => number }> {
  return watch(read, { derived: true });
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

  it('has no snapshot after a failed first load, until a read lands', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const store = await importStore();
    expect(store.hasSnapshot).toBe(false);
    const loading = store.load();
    fetches[0].answer.reject(new Error('backend down'));
    await loading;
    // Loaded, but the catalog is still the empty one the store starts with.
    expect(store.loaded).toBe(true);
    expect(store.hasSnapshot).toBe(false);
    expect(store.groups).toEqual([]);

    // A catalog write patched in is not the backend's catalog either.
    store.patchLocal((current) => current);
    expect(store.hasSnapshot).toBe(false);

    answer(1, withP1('done'));
    await tick();
    expect(store.hasSnapshot).toBe(true);
    expect(store.groups.map((g) => g.id)).toContain('status');
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

describe('identity across snapshots', () => {
  /** Two projects in Status, one ungrouped tag on p1, and a second group. */
  function board(edit: (draft: TagState) => void = () => {}): TagState {
    const draft: TagState = {
      groups: [
        ...state().groups,
        {
          id: 'area',
          name: 'Area',
          position: 1,
          isIconGroup: false,
          showInSidebar: true,
          createdAt: 0,
          updatedAt: 0,
        },
      ],
      tags: [...state().tags, { ...tag('infra', 0), groupId: null }],
      assignments: [
        { projectId: 'p1', tagId: 'idea' },
        { projectId: 'p1', tagId: 'infra' },
        { projectId: 'p2', tagId: 'done' },
      ],
    };
    edit(draft);
    return draft;
  }

  async function loadedStore(initial: TagState = board()): Promise<Store> {
    const store = await importStore();
    const loading = store.load();
    answer(0, initial);
    await loading;
    return store;
  }

  /** Refetch, answering with `data` (new objects, as every fetch returns). */
  async function refetch(store: Store, data: TagState): Promise<void> {
    const refreshing = store.requestRefresh();
    answer(fetches.length - 1, data);
    await refreshing;
  }

  /** What the surfaces read, each counted on its own. */
  async function watchAll(store: Store) {
    return {
      snapshot: await watch(() => store.snapshot),
      catalog: await watch(() => store.catalog),
      projectTagIds: await watch(() => store.projectTagIds),
      usageCount: await watch(() => store.usageCount),
      // What each board card derives for its icon.
      p1Icon: await watchDerived(() => store.iconTagFor('p1')),
      p2Icon: await watchDerived(() => store.iconTagFor('p2')),
    };
  }

  function wakesOf(watched: Record<string, { wakes: () => number }>) {
    return Object.fromEntries(Object.entries(watched).map(([key, w]) => [key, w.wakes()]));
  }

  it('applies the first snapshot with the objects fetched', async () => {
    const initial = board();
    const store = await loadedStore(initial);
    expect(store.snapshot).toEqual(board());
    store.snapshot.tags.forEach((held, i) => expect(held).toBe(initial.tags[i]));
    store.snapshot.groups.forEach((held, i) => expect(held).toBe(initial.groups[i]));
    store.snapshot.assignments.forEach((held, i) => expect(held).toBe(initial.assignments[i]));
    expect(store.loaded).toBe(true);
  });

  it('keeps the snapshot, and wakes no reader, when a refetch changed nothing', async () => {
    const store = await loadedStore();
    const before = {
      snapshot: store.snapshot,
      catalog: store.catalog,
      projectTagIds: store.projectTagIds,
      p1Tags: store.tagIdsFor('p1'),
      p1Icon: store.iconTagFor('p1'),
    };
    const watched = await watchAll(store);

    await refetch(store, board());

    expect(store.snapshot).toBe(before.snapshot);
    expect(store.catalog).toBe(before.catalog);
    expect(store.projectTagIds).toBe(before.projectTagIds);
    expect(store.tagIdsFor('p1')).toBe(before.p1Tags);
    expect(store.iconTagFor('p1')).toBe(before.p1Icon);
    expect(wakesOf(watched)).toEqual({
      snapshot: 0,
      catalog: 0,
      projectTagIds: 0,
      usageCount: 0,
      p1Icon: 0,
      p2Icon: 0,
    });
  });

  it('settles a first load that fetched an empty catalog', async () => {
    const store = await importStore();
    const empty = store.snapshot;
    const loading = store.load();
    answer(0, { groups: [], tags: [], assignments: [] });
    await loading;
    expect(store.loaded).toBe(true);
    expect(store.snapshot).toBe(empty);
  });

  it('still ranks an unchanged snapshot above the responses requested before it', async () => {
    const store = await importStore();
    const empty = store.snapshot;
    // The startup load and a refresh are both out; the newer answers first,
    // with the empty catalog the store already holds.
    const older = store.load();
    const newer = store.requestRefresh();
    answer(1, { groups: [], tags: [], assignments: [] });
    await newer;
    expect(store.loaded).toBe(true);

    // The older response predates it, so it is still ignored.
    answer(0, board());
    await older;
    expect(store.snapshot).toBe(empty);
    expect(store.tagsById.size).toBe(0);
  });

  it('keeps every tag and group object when a project is retagged', async () => {
    // A card moved in another window: p2 goes from done to blocked.
    const store = await loadedStore();
    const before = store.snapshot;
    const tags = new Map(store.tagsById);
    const groups = [...store.groups];
    const p1Assignments = before.assignments.filter((a) => a.projectId === 'p1');

    await refetch(
      store,
      board((draft) => {
        draft.assignments[2] = { projectId: 'p2', tagId: 'blocked' };
      })
    );

    expect(store.iconTagFor('p2')?.id).toBe('blocked');
    // The lists the fetch left alone are the ones held, not copies...
    expect(store.snapshot.tags).toBe(before.tags);
    expect(store.snapshot.groups).toBe(before.groups);
    // ...so every surface is handed the tag and group objects it already has,
    expect(store.iconTagFor('p2')).toBe(tags.get('blocked'));
    expect(store.iconTagFor('p1')).toBe(tags.get('idea'));
    const p1Chips = store.chipTagsFor('p1');
    expect(p1Chips.map((chip) => chip.id)).toEqual(['idea', 'infra']);
    p1Chips.forEach((chip) => expect(chip).toBe(tags.get(chip.id)));
    for (const [id, held] of tags) expect(store.tagsById.get(id)).toBe(held);
    store.groups.forEach((group, i) => expect(group).toBe(groups[i]));
    // and the other project's assignments are the objects held too.
    expect(store.snapshot.assignments.filter((a) => a.projectId === 'p1')).toEqual(p1Assignments);
    store.snapshot.assignments
      .filter((a) => a.projectId === 'p1')
      .forEach((assignment, i) => expect(assignment).toBe(p1Assignments[i]));
    expect(store.snapshot.assignments).not.toBe(before.assignments);
  });

  it('does not wake the other project’s icon when a project is retagged', async () => {
    const store = await loadedStore();
    const watched = await watchAll(store);

    await refetch(
      store,
      board((draft) => {
        draft.assignments[2] = { projectId: 'p2', tagId: 'blocked' };
      })
    );

    // p1's card re-derives (the assignments are one value), but it is given
    // the tag object it had, so what reads its icon is left alone.
    expect(wakesOf(watched)).toMatchObject({ snapshot: 1, projectTagIds: 1, p1Icon: 0, p2Icon: 1 });
  });

  it('hands out a new object for a changed tag only', async () => {
    const store = await loadedStore();
    const tags = new Map(store.tagsById);
    const watched = await watchAll(store);

    const renamed = board((draft) => {
      draft.tags[1] = { ...draft.tags[1], name: 'Shipped', updatedAt: 9 };
    });
    await refetch(store, renamed);

    expect(store.tagsById.get('done')).toBe(renamed.tags[1]);
    expect(store.tagsById.get('done')?.name).toBe('Shipped');
    for (const id of ['idea', 'blocked', 'infra']) {
      expect(store.tagsById.get(id)).toBe(tags.get(id));
    }
    // p2 carries the renamed tag; p1 does not.
    expect(wakesOf(watched)).toMatchObject({ catalog: 1, p1Icon: 0, p2Icon: 1 });
  });

  it('hands out a new object for a changed group only', async () => {
    const store = await loadedStore();
    const before = store.snapshot;
    const [status, area] = store.groups;

    const pinned = board((draft) => {
      draft.groups[0] = { ...draft.groups[0], showInSidebar: true, updatedAt: 9 };
    });
    await refetch(store, pinned);

    expect(store.groups[0]).toBe(pinned.groups[0]);
    expect(store.groups[0]).not.toBe(status);
    expect(store.sidebarGroups.map((group) => group.id)).toEqual(['status', 'area']);
    expect(store.groups[1]).toBe(area);
    expect(store.snapshot.tags).toBe(before.tags);
    expect(store.snapshot.assignments).toBe(before.assignments);
  });

  it('follows added and removed rows', async () => {
    const store = await loadedStore();
    const tags = new Map(store.tagsById);
    const area = store.groups[1];

    await refetch(
      store,
      board((draft) => {
        // Status is deleted with its tags; Area gains a tag; infra stays.
        draft.groups = [{ ...draft.groups[1], position: 0, isIconGroup: true }];
        draft.tags = [draft.tags[3], { ...tag('ui', 0), groupId: 'area' }];
        draft.assignments = [
          { projectId: 'p2', tagId: 'ui' },
          { projectId: 'p1', tagId: 'infra' },
        ];
      })
    );

    expect(store.groups.map((group) => group.id)).toEqual(['area']);
    expect(store.groups[0]).not.toBe(area);
    expect([...store.tagsById.keys()].sort()).toEqual(['infra', 'ui']);
    expect(store.tagsById.get('infra')).toBe(tags.get('infra'));
    expect(store.iconTagFor('p2')?.id).toBe('ui');
    expect(store.iconTagFor('p1')).toBeNull();
    expect([...store.tagIdsFor('p1')]).toEqual(['infra']);
  });

  it('follows reordered rows', async () => {
    const store = await loadedStore();
    const tags = new Map(store.tagsById);

    // Status reordered to done, idea, blocked, and the lists read back in
    // another order.
    await refetch(
      store,
      board((draft) => {
        const [idea, done, blocked, infra] = draft.tags;
        draft.tags = [infra, { ...done, position: 0 }, { ...idea, position: 1 }, blocked];
        draft.groups.reverse();
        draft.assignments.reverse();
      })
    );

    expect(store.tagsByGroup.get('status')!.map((t) => t.id)).toEqual(['done', 'idea', 'blocked']);
    expect(store.snapshot.tags.map((t) => t.id)).toEqual(['infra', 'done', 'idea', 'blocked']);
    expect(store.snapshot.groups.map((group) => group.id)).toEqual(['area', 'status']);
    expect(store.snapshot.assignments).toEqual([...board().assignments].reverse());
    for (const id of ['blocked', 'infra']) expect(store.tagsById.get(id)).toBe(tags.get(id));
    for (const id of ['idea', 'done']) expect(store.tagsById.get(id)).not.toBe(tags.get(id));
    expect(store.iconTagFor('p1')?.id).toBe('idea');
    expect(store.iconTagFor('p2')?.id).toBe('done');
  });

  it('reconciles the network’s answer with the cached value shown first', async () => {
    const store = await importStore();
    const revalidated = deferred<TagState>();
    const loading = store.load();
    fetches[0].answer.resolve({ data: board(), revalidating: revalidated.promise });
    await tick();
    const shown = store.snapshot;
    expect(shown).toEqual(board());
    const watched = await watchAll(store);

    // The network agrees with the cache: nothing to repaint.
    revalidated.resolve(board());
    await loading;
    expect(store.snapshot).toBe(shown);
    expect(wakesOf(watched)).toMatchObject({ snapshot: 0, catalog: 0, projectTagIds: 0 });
  });

  it('reconciles against a local patch, keeping what the patch got right', async () => {
    const store = await loadedStore();
    const created = { ...tag('shipping', 3), updatedAt: 4 };
    store.patchLocal((s) => ({ ...s, tags: [...s.tags, created] }));
    const patched = store.snapshot;
    expect(store.tagsById.get('shipping')).toBe(created);
    const watched = await watchAll(store);

    // The refetch patchLocal requested confirms the patch.
    await tick();
    answer(
      fetches.length - 1,
      board((draft) => {
        draft.tags.push({ ...tag('shipping', 3), updatedAt: 4 });
      })
    );
    await tick();

    expect(store.snapshot).toBe(patched);
    expect(store.tagsById.get('shipping')).toBe(created);
    expect(wakesOf(watched)).toMatchObject({ snapshot: 0, catalog: 0 });
  });

  describe('with pending writes', () => {
    it('clears a confirmed pick once its echo lands, with the tag objects kept', async () => {
      const store = await loadedStore();
      const tags = new Map(store.tagsById);
      const { slot, token } = store.beginIntent('p1', {
        kind: 'group',
        groupId: 'status',
        tagId: 'done',
      });
      expect(store.iconTagFor('p1')).toBe(tags.get('done'));
      store.confirmIntent('p1', slot, token);

      // The echo: the pick is in the snapshot now.
      answer(
        fetches.length - 1,
        board((draft) => {
          draft.assignments[0] = { projectId: 'p1', tagId: 'done' };
        })
      );
      await tick();
      expect(store.iconTagFor('p1')).toBe(tags.get('done'));

      // Server truth, not the overlay: a later change shows through.
      await refetch(
        store,
        board((draft) => {
          draft.assignments[0] = { projectId: 'p1', tagId: 'blocked' };
        })
      );
      expect(store.iconTagFor('p1')).toBe(tags.get('blocked'));
    });

    it('rolls a failed pick back when the refetch changed nothing', async () => {
      const store = await loadedStore();
      const before = store.snapshot;
      const idea = store.tagsById.get('idea');
      const watched = await watchAll(store);
      const { slot, token } = store.beginIntent('p1', {
        kind: 'group',
        groupId: 'status',
        tagId: 'done',
      });
      expect(store.iconTagFor('p1')?.id).toBe('done');
      expect(wakesOf(watched)).toMatchObject({ snapshot: 0, p1Icon: 1, p2Icon: 0 });

      // The write failed, so the server still holds what the store does.
      store.dropIntent('p1', slot, token);
      expect(store.iconTagFor('p1')).toBe(idea);
      expect(wakesOf(watched)).toMatchObject({ snapshot: 0, p1Icon: 1, p2Icon: 0 });

      answer(fetches.length - 1, board());
      await tick();
      expect(store.snapshot).toBe(before);
      expect(store.iconTagFor('p1')).toBe(idea);
      expect(wakesOf(watched)).toMatchObject({ snapshot: 0, p1Icon: 0, p2Icon: 0 });
    });

    it('drops a confirmed pick the server undid, though the snapshot is the one held', async () => {
      // Picked done, then another window put idea back before the echo was
      // read: the snapshot equals the one held, and the pick must still go.
      const store = await loadedStore();
      const before = store.snapshot;
      const { slot, token } = store.beginIntent('p1', {
        kind: 'group',
        groupId: 'status',
        tagId: 'done',
      });
      store.confirmIntent('p1', slot, token);
      expect(store.iconTagFor('p1')?.id).toBe('done');

      answer(fetches.length - 1, board());
      await tick();

      expect(store.snapshot).toBe(before);
      expect(store.iconTagFor('p1')?.id).toBe('idea');
    });

    it('keeps a pending pick over an unchanged snapshot', async () => {
      const store = await loadedStore();
      store.beginIntent('p1', { kind: 'group', groupId: 'status', tagId: 'done' });
      await refetch(store, board());
      expect(store.iconTagFor('p1')?.id).toBe('done');
      expect(store.iconTagFor('p2')?.id).toBe('done');
    });
  });
});
