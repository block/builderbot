// Runs in the "component" vitest project: the sorted list, the filters, the
// sort preference, the activity store, the real tagsStore and the pending
// catalog overlay hold $state/$derived fields that only compile for the
// client. The backend commands, the change feed and the preference store are
// mocked, and the project list is rune state the tests own.
//
// This is the grid-and-sidebar agreement test, at the one place the order is
// made: every surface renders `sortedProjects` or `filteredProjects`, so the
// two staying in one order through every kind of change is what keeps the
// grid, the sidebar and the boards from disagreeing.
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectActivity, TagState } from '../../types';
import { readReactively } from '../../stores/testStubs/readReactively.svelte';
import { withTagMoved } from '../tags/tagCatalogPatches';
import { tagState, withGroupTag } from '../tags/testStubs/tagFixtures';
import type { ProjectSort } from './projectSort';

type Handler = (payload: unknown) => void;

/** What the backend holds; every `get_tag_state` answers with it. */
let server: TagState;
/** What `get_project_activity` answers with. */
let activityRows: ProjectActivity[];
let getProjectActivity: ReturnType<typeof vi.fn>;
let moveTag: ReturnType<typeof vi.fn>;
let setStoreValue: ReturnType<typeof vi.fn>;
/** What preferences.json holds when the window opens. */
let storedSort: unknown;
let handlers: Map<string, Handler[]>;
let unreadIds: Set<string>;
let stops: Array<() => void>;

function project(id: string, createdAt: number): Project {
  return {
    id,
    name: id,
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt,
    updatedAt: createdAt,
  };
}

// Newest first, as the backend lists them.
const a = project('a', 400);
const b = project('b', 300);
const c = project('c', 200);
const d = project('d', 100);

// Status (icon group: idea, doing, done), Priority (p0, p1), ungrouped q4, infra.
const ASSIGNMENTS: TagState['assignments'] = [
  { projectId: 'a', tagId: 'p1' },
  { projectId: 'b', tagId: 'q4' },
  { projectId: 'c', tagId: 'p0' },
  { projectId: 'c', tagId: 'q4' },
  { projectId: 'd', tagId: 'p1' },
  { projectId: 'd', tagId: 'q4' },
];

const BY_PRIORITY: ProjectSort = [{ kind: 'group', groupId: 'priority', direction: 'asc' }];
const BY_UPDATED: ProjectSort = [{ kind: 'updated', direction: 'desc' }];

function emit(event: string): void {
  for (const handler of handlers.get(event) ?? []) handler({});
}

/** Let promises, the store's refetches and the root effect run. */
async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
  flushSync();
}

const ids = (projects: readonly Project[]) => projects.map((p) => p.id);

async function setup({
  assignments = ASSIGNMENTS,
}: { assignments?: TagState['assignments'] } = {}) {
  server = tagState(assignments);
  const { world } = await import('./testStubs/liveProjectsWorld.svelte');
  world.projects = [a, b, c, d];
  const { tagsStore } = await import('../../stores/tags.svelte');
  const { tagActions } = await import('../tags/tagActions.svelte');
  const { pendingCatalogMoves } = await import('../tags/pendingCatalogMoves.svelte');
  const prefs = await import('./projectSortPrefs.svelte');
  const { projectActivity, REFETCH_DELAY_MS } = await import('./projectActivity.svelte');
  const { projectOrderStore } = await import('./projectOrder.svelte');
  const { projectFiltersStore } = await import('./projectFilters.svelte');
  await prefs.loadProjectSortPrefs();
  await tagsStore.load();
  stops.push(() => projectActivity.stop());

  /** Every array each list handed a reader, as a template would see them. */
  const sortedSeen: Project[][] = [];
  const filteredSeen: Project[][] = [];
  stops.push(readReactively(() => sortedSeen.push(projectOrderStore.sortedProjects)));
  stops.push(readReactively(() => filteredSeen.push(projectFiltersStore.filteredProjects)));
  await settle();

  return {
    world,
    tagsStore,
    tagActions,
    pendingCatalogMoves,
    projectFiltersStore,
    projectOrderStore,
    setProjectSort: prefs.setProjectSort,
    REFETCH_DELAY_MS,
    sortedSeen,
    filteredSeen,
    sorted: () => ids(projectOrderStore.sortedProjects),
    filtered: () => ids(projectFiltersStore.filteredProjects),
    /** Land a refetch with `rows`, as a write on the change feed would. */
    async activityChanged(rows: ProjectActivity[]) {
      activityRows = rows;
      emit('notes-changed');
      await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS);
      flushSync();
    },
  };
}

/** The filtered list is the sorted one minus what the filters rule out, in the same order. */
function expectSameOrder(sorted: string[], filtered: string[]): void {
  expect(sorted.filter((id) => filtered.includes(id))).toEqual(filtered);
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  activityRows = [];
  storedSort = undefined;
  handlers = new Map();
  unreadIds = new Set();
  stops = [];
  getProjectActivity = vi.fn(async () => structuredClone(activityRows));
  moveTag = vi.fn(async (tagId: string, groupId: string | null, index: number) => {
    server = withTagMoved(server, { tagId, groupId, index });
  });
  setStoreValue = vi.fn(async () => {});

  const commandsMock = {
    getTagState: vi.fn(async () => ({ data: structuredClone(server), revalidating: null })),
    getProjectActivity,
    moveTag,
    setProjectGroupTag: vi.fn(async (projectId: string, groupId: string, tagId: string | null) => {
      server = withGroupTag(server, projectId, groupId, tagId);
    }),
  };
  vi.doMock('../../commands', () => commandsMock);
  vi.doMock('../../api/commands', () => commandsMock);
  vi.doMock('../../transport', () => ({
    listenToEvent: (event: string, handler: Handler) => {
      handlers.set(event, [...(handlers.get(event) ?? []), handler]);
      return () => {};
    },
  }));
  vi.doMock('../../shared/persistentStore', () => ({
    getStoreValue: vi.fn(async (key: string) => (key === 'projects-sort' ? storedSort : undefined)),
    setStoreValue,
  }));
  vi.doMock('../../stores/projectsData.svelte', async () => {
    const { world } = await import('./testStubs/liveProjectsWorld.svelte');
    return {
      projectsDataStore: {
        get projects() {
          return world.projects;
        },
        get liveProjects() {
          return world.liveProjects;
        },
        reposByProject: new Map(),
        branchesByProject: new Map(),
        deletingProjectNames: new Map(),
        isProjectDeleting: (projectId: string) => world.deleting.has(projectId),
      },
    };
  });
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: { isUnread: (projectId: string) => unreadIds.has(projectId) },
  }));
  vi.doMock('./projectStatus', () => ({
    getProjectStatus: () => ({ kind: 'idle', runningCount: 0, runActionPhase: null }),
  }));
  vi.doMock('./workspaceLifecycle.svelte', () => ({
    workspaceLifecycle: { getWorktreeErrors: () => new Map() },
  }));
  vi.doMock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
});

afterEach(() => {
  for (const stop of stops) stop();
  vi.doUnmock('../../commands');
  vi.doUnmock('../../api/commands');
  vi.doUnmock('../../transport');
  vi.doUnmock('../../shared/persistentStore');
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('./projectStatus');
  vi.doUnmock('./workspaceLifecycle.svelte');
  vi.doUnmock('svelte-sonner');
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('with no sort, the default', () => {
  it('both lists are the live list itself, newest first', async () => {
    const { world, projectOrderStore, projectFiltersStore } = await setup();

    expect(projectOrderStore.sortedProjects).toBe(world.liveProjects);
    expect(projectFiltersStore.filteredProjects).toBe(world.liveProjects);
    expect(ids(projectOrderStore.sortedProjects)).toEqual(['a', 'b', 'c', 'd']);
  });

  it('nothing recomputes when tags, tag order or the change feed move', async () => {
    const { tagActions, pendingCatalogMoves, sortedSeen, filteredSeen, REFETCH_DELAY_MS } =
      await setup();

    void tagActions.setProjectGroupTag(a, 'priority', 'p0');
    void pendingCatalogMoves.moveTag({ tagId: 'p1', groupId: 'priority', index: 0 });
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS * 2);
    flushSync();

    // One array each, handed out once: no reader ran again.
    expect(sortedSeen).toHaveLength(1);
    expect(filteredSeen).toHaveLength(1);
    // And the activity read was never made.
    expect(getProjectActivity).not.toHaveBeenCalled();
    expect(handlers.has('notes-changed')).toBe(false);
  });

  it('keeps following the list: a new project leads, a deleting one goes', async () => {
    const { world, projectOrderStore, sorted, filtered } = await setup();
    const e = project('e', 500);

    world.projects = [e, ...world.projects];
    flushSync();
    expect(projectOrderStore.sortedProjects).toBe(world.projects);
    expect(filtered()).toEqual(['e', 'a', 'b', 'c', 'd']);

    world.deleting = new Set(['b']);
    flushSync();
    expect(sorted()).toEqual(['e', 'a', 'c', 'd']);
    expect(filtered()).toEqual(['e', 'a', 'c', 'd']);
  });

  it('a sort naming only a group since deleted is no sort', async () => {
    storedSort = [{ kind: 'group', groupId: 'gone', direction: 'asc' }];
    const { world, projectOrderStore, projectFiltersStore } = await setup();

    expect(projectOrderStore.sortedProjects).toBe(world.liveProjects);
    expect(projectFiltersStore.filteredProjects).toBe(world.liveProjects);
    // Read, not rewritten: the dead key goes with the next edit.
    expect(setStoreValue).not.toHaveBeenCalled();
  });
});

describe('with a sort', () => {
  it('starts in the stored sort', async () => {
    storedSort = BY_PRIORITY;
    const { sorted, filtered } = await setup();

    // c is P0; a and d are P1, newest first; b has no priority and goes last.
    expect(sorted()).toEqual(['c', 'a', 'd', 'b']);
    expect(filtered()).toEqual(['c', 'a', 'd', 'b']);
  });

  it('both lists take a new sort at once, and go back when it is cleared', async () => {
    const { world, projectOrderStore, setProjectSort, sorted, filtered } = await setup();

    setProjectSort(BY_PRIORITY);
    flushSync();
    expect(sorted()).toEqual(['c', 'a', 'd', 'b']);
    expect(filtered()).toEqual(['c', 'a', 'd', 'b']);

    setProjectSort([{ kind: 'group', groupId: 'priority', direction: 'desc' }]);
    flushSync();
    // Flipped, with the untagged project still last.
    expect(sorted()).toEqual(['a', 'd', 'c', 'b']);
    expect(filtered()).toEqual(['a', 'd', 'c', 'b']);

    setProjectSort([
      { kind: 'group', groupId: 'priority', direction: 'asc' },
      { kind: 'created', direction: 'asc' },
    ]);
    flushSync();
    expect(sorted()).toEqual(['c', 'd', 'a', 'b']);
    expect(filtered()).toEqual(['c', 'd', 'a', 'b']);

    setProjectSort([]);
    flushSync();
    expect(projectOrderStore.sortedProjects).toBe(world.liveProjects);
    expect(filtered()).toEqual(['a', 'b', 'c', 'd']);
  });

  it('the filters narrow the sorted list without reordering it', async () => {
    storedSort = BY_PRIORITY;
    const { projectFiltersStore, sorted, filtered } = await setup();

    projectFiltersStore.toggleFilter({ tagId: 'q4' });
    flushSync();
    expect(filtered()).toEqual(['c', 'd', 'b']);
    expectSameOrder(sorted(), filtered());

    unreadIds = new Set(['a', 'd', 'b']);
    projectFiltersStore.toggleFilter('unread');
    flushSync();
    expect(filtered()).toEqual(['a', 'd', 'b']);
    expectSameOrder(sorted(), filtered());

    projectFiltersStore.clearFilters();
    flushSync();
    expect(filtered()).toEqual(sorted());
  });

  it('a tag picked on a project moves it in both lists before the write lands', async () => {
    storedSort = BY_PRIORITY;
    const { tagActions, projectFiltersStore, sorted, filtered } = await setup();
    projectFiltersStore.toggleFilter({ tagId: 'q4' });
    flushSync();
    expect(filtered()).toEqual(['c', 'd', 'b']);

    // b gets P0: it joins c at the front, newest first.
    void tagActions.setProjectGroupTag(b, 'priority', 'p0');
    flushSync();
    expect(sorted()).toEqual(['b', 'c', 'a', 'd']);
    expect(filtered()).toEqual(['b', 'c', 'd']);

    await settle();
    expect(sorted()).toEqual(['b', 'c', 'a', 'd']);
    expect(filtered()).toEqual(['b', 'c', 'd']);

    // And cleared again: back to the end, untagged.
    void tagActions.setProjectGroupTag(b, 'priority', null);
    await settle();
    expect(sorted()).toEqual(['c', 'a', 'd', 'b']);
    expect(filtered()).toEqual(['c', 'd', 'b']);
  });

  it('a tag reorder re-sorts both lists before its write lands', async () => {
    storedSort = BY_PRIORITY;
    let land!: () => void;
    moveTag.mockImplementationOnce(
      (tagId: string, groupId: string | null, index: number) =>
        new Promise<void>((resolve) => {
          land = () => {
            server = withTagMoved(server, { tagId, groupId, index });
            resolve();
          };
        })
    );
    const { pendingCatalogMoves, sorted, filtered } = await setup();
    expect(sorted()).toEqual(['c', 'a', 'd', 'b']);

    // P1 dragged ahead of P0, on a board or in Settings › Tags.
    void pendingCatalogMoves.moveTag({ tagId: 'p1', groupId: 'priority', index: 0 });
    flushSync();
    expect(sorted()).toEqual(['a', 'd', 'c', 'b']);
    expect(filtered()).toEqual(['a', 'd', 'c', 'b']);

    await settle();
    expect(moveTag).toHaveBeenCalledTimes(1);
    land();
    await settle();
    expect(sorted()).toEqual(['a', 'd', 'c', 'b']);
    expect(filtered()).toEqual(['a', 'd', 'c', 'b']);
  });

  it('a change that moves no project hands both readers nothing new', async () => {
    storedSort = BY_PRIORITY;
    const { tagActions, sortedSeen, filteredSeen, sorted } = await setup();
    const sortedReads = sortedSeen.length;
    const filteredReads = filteredSeen.length;

    // A tag outside the sort's group: every project stays where it is.
    void tagActions.setProjectGroupTag(a, 'status', 'doing');
    await settle();

    expect(sorted()).toEqual(['c', 'a', 'd', 'b']);
    expect(sortedSeen).toHaveLength(sortedReads);
    expect(filteredSeen).toHaveLength(filteredReads);
  });

  it('a new project takes its sorted place and a deleting one leaves both lists', async () => {
    storedSort = BY_PRIORITY;
    const { world, sorted, filtered } = await setup();

    // Created in this window: prepended to the list, untagged.
    world.projects = [project('e', 500), ...world.projects];
    flushSync();
    expect(sorted()).toEqual(['c', 'a', 'd', 'e', 'b']);
    expect(filtered()).toEqual(['c', 'a', 'd', 'e', 'b']);

    world.deleting = new Set(['a']);
    flushSync();
    expect(sorted()).toEqual(['c', 'd', 'e', 'b']);
    expect(filtered()).toEqual(['c', 'd', 'e', 'b']);

    // The delete failed: it comes back where it was.
    world.deleting = new Set();
    flushSync();
    expect(sorted()).toEqual(['c', 'a', 'd', 'e', 'b']);
  });

  it('never reads activity for a sort without a last-updated key', async () => {
    storedSort = BY_PRIORITY;
    const { REFETCH_DELAY_MS } = await setup();

    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS * 2);

    expect(getProjectActivity).not.toHaveBeenCalled();
  });
});

describe('sorted by last updated', () => {
  it('loads the activity once the sort asks for it, and orders both lists by it', async () => {
    // d was worked on last, then b; a and c only have their creation times.
    activityRows = [
      { projectId: 'd', lastActivityAt: 900 },
      { projectId: 'b', lastActivityAt: 350 },
    ];
    const { setProjectSort, sorted, filtered } = await setup();
    expect(getProjectActivity).not.toHaveBeenCalled();

    setProjectSort(BY_UPDATED);
    flushSync();
    // Until the read lands, last updated is the creation time.
    expect(sorted()).toEqual(['a', 'b', 'c', 'd']);

    await settle();
    expect(getProjectActivity).toHaveBeenCalledTimes(1);
    expect(sorted()).toEqual(['d', 'a', 'b', 'c']);
    expect(filtered()).toEqual(['d', 'a', 'b', 'c']);
  });

  it('loads it at startup when the stored sort has the key', async () => {
    storedSort = BY_UPDATED;
    activityRows = [{ projectId: 'c', lastActivityAt: 900 }];
    const { sorted, filtered } = await setup();

    expect(getProjectActivity).toHaveBeenCalledTimes(1);
    expect(sorted()).toEqual(['c', 'a', 'b', 'd']);
    expect(filtered()).toEqual(['c', 'a', 'b', 'd']);
  });

  it('re-sorts both lists live when work lands on a project', async () => {
    storedSort = BY_UPDATED;
    const { projectFiltersStore, activityChanged, sorted, filtered } = await setup();
    projectFiltersStore.toggleFilter({ tagId: 'q4' });
    flushSync();
    expect(sorted()).toEqual(['a', 'b', 'c', 'd']);
    expect(filtered()).toEqual(['b', 'c', 'd']);

    // A session on d writes its note.
    await activityChanged([{ projectId: 'd', lastActivityAt: 900 }]);
    expect(sorted()).toEqual(['d', 'a', 'b', 'c']);
    expect(filtered()).toEqual(['d', 'b', 'c']);

    // Then one finishes on c.
    await activityChanged([
      { projectId: 'd', lastActivityAt: 900 },
      { projectId: 'c', lastActivityAt: 950 },
    ]);
    expect(sorted()).toEqual(['c', 'd', 'a', 'b']);
    expect(filtered()).toEqual(['c', 'd', 'b']);
  });

  it('activity that moves no project hands both readers nothing new', async () => {
    storedSort = BY_UPDATED;
    activityRows = [{ projectId: 'd', lastActivityAt: 900 }];
    const { activityChanged, sortedSeen, filteredSeen, sorted } = await setup();
    const sortedReads = sortedSeen.length;
    const filteredReads = filteredSeen.length;

    // d, already first, is worked on again; a's first note is older than b.
    await activityChanged([
      { projectId: 'd', lastActivityAt: 990 },
      { projectId: 'a', lastActivityAt: 410 },
    ]);

    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(sorted()).toEqual(['d', 'a', 'b', 'c']);
    expect(sortedSeen).toHaveLength(sortedReads);
    expect(filteredSeen).toHaveLength(filteredReads);
  });

  it('composes with a tag key, each list in the same order', async () => {
    storedSort = [...BY_PRIORITY, ...BY_UPDATED];
    activityRows = [{ projectId: 'd', lastActivityAt: 900 }];
    const { tagActions, sorted, filtered } = await setup();

    // P0: c. P1: d (updated last), then a. Untagged: b.
    expect(sorted()).toEqual(['c', 'd', 'a', 'b']);
    expect(filtered()).toEqual(['c', 'd', 'a', 'b']);

    void tagActions.setProjectGroupTag(b, 'priority', 'p1');
    flushSync();
    expect(sorted()).toEqual(['c', 'd', 'a', 'b']);
    void tagActions.setProjectGroupTag(d, 'priority', null);
    flushSync();
    expect(sorted()).toEqual(['c', 'a', 'b', 'd']);
    expect(filtered()).toEqual(['c', 'a', 'b', 'd']);
  });

  it('holds a re-sort while a pointer is down, and applies it after the release', async () => {
    storedSort = BY_UPDATED;
    const { activityChanged, sorted, filtered } = await setup();

    window.dispatchEvent(new Event('pointerdown'));
    await activityChanged([{ projectId: 'd', lastActivityAt: 900 }]);
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(sorted()).toEqual(['a', 'b', 'c', 'd']);
    expect(filtered()).toEqual(['a', 'b', 'c', 'd']);

    window.dispatchEvent(new Event('pointerup'));
    window.dispatchEvent(new Event('click'));
    // The click has been dispatched with the rows where they were pressed.
    flushSync();
    expect(sorted()).toEqual(['a', 'b', 'c', 'd']);

    await settle();
    expect(sorted()).toEqual(['d', 'a', 'b', 'c']);
    expect(filtered()).toEqual(['d', 'a', 'b', 'c']);
  });

  it('stops reading once the key is removed, and catches up when it comes back', async () => {
    storedSort = BY_UPDATED;
    const { setProjectSort, sorted, REFETCH_DELAY_MS } = await setup();
    expect(getProjectActivity).toHaveBeenCalledTimes(1);

    setProjectSort([]);
    flushSync();
    activityRows = [{ projectId: 'd', lastActivityAt: 900 }];
    emit('notes-changed');
    await vi.advanceTimersByTimeAsync(REFETCH_DELAY_MS * 2);
    expect(getProjectActivity).toHaveBeenCalledTimes(1);
    expect(sorted()).toEqual(['a', 'b', 'c', 'd']);

    setProjectSort(BY_UPDATED);
    await settle();
    expect(getProjectActivity).toHaveBeenCalledTimes(2);
    expect(sorted()).toEqual(['d', 'a', 'b', 'c']);
  });
});
