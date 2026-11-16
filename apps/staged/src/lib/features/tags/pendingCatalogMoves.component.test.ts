// Runs in the "component" vitest project: the overlay, tagActions and the
// real tagsStore hold $state/$derived fields that only compile for the
// client. The assertions are on the catalog a surface would render and on
// which commands reached the backend.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TagState } from '../../types';
import { withGroupOrder, withTagMoved } from './tagCatalogPatches';
import type { TagCatalog } from './tagState';
import { group, scopeIds, tagState } from './testStubs/tagFixtures';

/** A backend write the test lands or fails by hand. */
interface HeldWrite {
  land: () => void;
  fail: (reason: Error) => void;
}

/** What the backend holds; every `get_tag_state` answers with it. */
let server: TagState;
/** The moves and reorders the backend has been sent, oldest first. */
let writes: HeldWrite[];
let moveTag: ReturnType<typeof vi.fn>;
let reorderTagGroups: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;

/** Hold a write open; landing it applies `commit` to the backend's state. */
function held(commit: (state: TagState) => TagState): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    writes.push({
      land: () => {
        server = commit(server);
        resolve();
      },
      fail: reject,
    });
  });
}

/** Let the catalog chain's steps and the store's refetches run. */
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function statusIds(catalog: TagCatalog): string[] {
  return (catalog.tagsByGroup.get('status') ?? []).map((tag) => tag.id);
}

function groupIds(catalog: TagCatalog): string[] {
  return catalog.groups.map((g) => g.id);
}

/** Status, Priority and Area, so a group reorder has somewhere to go. */
function threeGroups(): TagState {
  const state = tagState();
  return { ...state, groups: [...state.groups, group('area', 2, { name: 'Area' })] };
}

async function setup(initial: TagState = tagState()) {
  server = initial;
  const { tagsStore } = await import('../../stores/tags.svelte');
  const { pendingCatalogMoves } = await import('./pendingCatalogMoves.svelte');
  await tagsStore.load();
  return { tagsStore, pendingCatalogMoves };
}

beforeEach(() => {
  vi.resetModules();
  writes = [];
  toastError = vi.fn();
  moveTag = vi.fn((tagId: string, groupId: string | null, index: number) =>
    held((state) => withTagMoved(state, { tagId, groupId, index }))
  );
  reorderTagGroups = vi.fn((ids: string[]) => held((state) => withGroupOrder(state, ids)));

  const commandsMock = {
    getTagState: vi.fn(async () => ({ data: structuredClone(server), revalidating: null })),
    moveTag,
    reorderTagGroups,
  };
  vi.doMock('../../commands', () => commandsMock);
  vi.doMock('../../api/commands', () => commandsMock);
  vi.doMock('../../transport', () => ({ listenToEvent: () => () => {} }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: { liveProjects: [], isProjectDeleting: () => false },
  }));
  vi.doMock('svelte-sonner', () => ({ toast: { error: toastError } }));
});

afterEach(() => {
  vi.doUnmock('../../commands');
  vi.doUnmock('../../api/commands');
  vi.doUnmock('../../transport');
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('svelte-sonner');
});

describe('pendingCatalogMoves', () => {
  it('follows the store while nothing is pending', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup();
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['idea', 'doing', 'done']);

    // Another window's write arrives as a refetch.
    server = withTagMoved(server, { tagId: 'done', groupId: 'status', index: 0 });
    await tagsStore.requestRefresh();
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['done', 'idea', 'doing']);
  });

  it('shows a tag move before its write lands, and the store holds it after', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup();

    const moved = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 2 });
    // Shown at once, in the overlay only: the store waits for the backend.
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'done', 'idea']);
    expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['idea', 'doing', 'done']);
    await settle();
    expect(moveTag.mock.calls).toEqual([['idea', 'status', 2]]);

    writes[0].land();
    expect(await moved).toBe(true);
    expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'done', 'idea']);
  });

  it('shows a tag move into another scope', async () => {
    const { pendingCatalogMoves } = await setup();

    void pendingCatalogMoves.moveTag({ tagId: 'q4', groupId: 'priority', index: 0 });
    const { tagsByGroup } = pendingCatalogMoves.catalog;
    expect(tagsByGroup.get('priority')?.map((tag) => tag.id)).toEqual(['q4', 'p0', 'p1']);
    expect(tagsByGroup.get(null)?.map((tag) => tag.id)).toEqual(['infra']);

    await settle();
    writes[0].land();
    await settle();
  });

  it('shows a group reorder before its write lands, and the store holds it after', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup(threeGroups());

    const order = ['area', 'status', 'priority'];
    const reordered = pendingCatalogMoves.reorderGroups(order);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(order);
    expect(groupIds(tagsStore.catalog)).toEqual(['status', 'priority', 'area']);
    await settle();
    expect(reorderTagGroups.mock.calls).toEqual([[order]]);

    writes[0].land();
    expect(await reordered).toBe(true);
    expect(groupIds(tagsStore.catalog)).toEqual(order);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(order);
  });

  it('copies the group order, so a caller reusing its array changes nothing', async () => {
    const { pendingCatalogMoves } = await setup(threeGroups());

    const order = ['area', 'status', 'priority'];
    void pendingCatalogMoves.reorderGroups(order);
    order.reverse();
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['area', 'status', 'priority']);
    await settle();
    expect(reorderTagGroups.mock.calls).toEqual([[['area', 'status', 'priority']]]);

    writes[0].land();
    await settle();
  });

  it('stacks entries of both kinds in order, sending them one at a time', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup(threeGroups());

    const first = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 1 });
    const second = pendingCatalogMoves.reorderGroups(['priority', 'status', 'area']);
    // The third is resolved against what the first left: one more step down.
    const third = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 2 });
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'done', 'idea']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['priority', 'status', 'area']);
    await settle();
    expect(moveTag).toHaveBeenCalledTimes(1);
    expect(reorderTagGroups).not.toHaveBeenCalled();

    // The first landing doesn't throw the other two away.
    writes[0].land();
    expect(await first).toBe(true);
    await settle();
    expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['doing', 'idea', 'done']);
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'done', 'idea']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['priority', 'status', 'area']);
    expect(reorderTagGroups).toHaveBeenCalledTimes(1);
    expect(moveTag).toHaveBeenCalledTimes(1);

    writes[1].land();
    expect(await second).toBe(true);
    await settle();
    expect(moveTag.mock.calls).toEqual([
      ['idea', 'status', 1],
      ['idea', 'status', 2],
    ]);

    writes[2].land();
    expect(await third).toBe(true);
    expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(groupIds(tagsStore.catalog)).toEqual(['priority', 'status', 'area']);
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'done', 'idea']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['priority', 'status', 'area']);
  });

  it('lets a landed entry go, so the catalog can diverge after it', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup();

    const moved = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 1 });
    await settle();
    writes[0].land();
    await moved;
    await settle();
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'idea', 'done']);

    // Another window puts Idea back on top. With the move still overlaid it
    // would be re-applied over the refetch and Idea would stay second.
    server = withTagMoved(server, { tagId: 'idea', groupId: 'status', index: 0 });
    await tagsStore.requestRefresh();
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['idea', 'doing', 'done']);
  });

  it('drops every entry when a write fails, and skips the ones queued behind it', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup(threeGroups());

    // As a tag move from one surface and a group reorder from another would
    // queue: there is one write chain, so one failure stops both.
    const first = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 2 });
    const second = pendingCatalogMoves.reorderGroups(['area', 'status', 'priority']);
    const third = pendingCatalogMoves.moveTag({ tagId: 'doing', groupId: 'status', index: 0 });
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['doing', 'done', 'idea']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['area', 'status', 'priority']);
    await settle();

    writes[0].fail(new Error('db locked'));
    expect(await Promise.all([first, second, third])).toEqual([false, false, false]);
    await settle();
    // Server truth is back, the two behind it (resolved against an order
    // that never happened) were never sent, and the one failure toasted.
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['idea', 'doing', 'done']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['status', 'priority', 'area']);
    expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['idea', 'doing', 'done']);
    expect(moveTag).toHaveBeenCalledTimes(1);
    expect(reorderTagGroups).not.toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledWith('Unable to move tag', { description: 'db locked' });
  });

  it('sends a reorder made after a failure, starting from the server’s order', async () => {
    const { tagsStore, pendingCatalogMoves } = await setup(threeGroups());

    const failed = pendingCatalogMoves.reorderGroups(['area', 'status', 'priority']);
    const skipped = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 2 });
    await settle();
    writes[0].fail(new Error('not a permutation'));
    expect(await Promise.all([failed, skipped])).toEqual([false, false]);
    expect(toastError).toHaveBeenCalledWith('Unable to reorder groups', {
      description: 'not a permutation',
    });
    expect(moveTag).not.toHaveBeenCalled();

    const retried = pendingCatalogMoves.reorderGroups(['priority', 'status', 'area']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['priority', 'status', 'area']);
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['idea', 'doing', 'done']);
    await settle();
    expect(reorderTagGroups.mock.calls).toEqual([
      [['area', 'status', 'priority']],
      [['priority', 'status', 'area']],
    ]);

    writes[1].land();
    expect(await retried).toBe(true);
    expect(groupIds(tagsStore.catalog)).toEqual(['priority', 'status', 'area']);
    expect(groupIds(pendingCatalogMoves.catalog)).toEqual(['priority', 'status', 'area']);
    expect(toastError).toHaveBeenCalledTimes(1);
  });

  it('keeps an entry made while a failed one’s skipped followers drain', async () => {
    const { pendingCatalogMoves } = await setup();

    const failed = pendingCatalogMoves.moveTag({ tagId: 'idea', groupId: 'status', index: 2 });
    const skipped = pendingCatalogMoves.moveTag({ tagId: 'doing', groupId: 'status', index: 0 });
    await settle();
    writes[0].fail(new Error('db locked'));
    await failed;
    // Made before the skipped move has settled: it belongs to the new
    // generation, so the skip's bookkeeping must leave it alone.
    const fresh = pendingCatalogMoves.moveTag({ tagId: 'done', groupId: 'status', index: 0 });
    expect(await skipped).toBe(false);
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['done', 'idea', 'doing']);
    await settle();
    expect(moveTag.mock.calls).toEqual([
      ['idea', 'status', 2],
      ['done', 'status', 0],
    ]);

    writes[1].land();
    expect(await fresh).toBe(true);
    expect(statusIds(pendingCatalogMoves.catalog)).toEqual(['done', 'idea', 'doing']);
  });
});
