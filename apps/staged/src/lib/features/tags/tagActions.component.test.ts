// Runs in the "component" vitest project so tagActions drives the real
// tagsStore, whose $state/$derived fields only compile for the client: the
// assertions are on what every surface would show (the store's effective
// tags), not on which store methods were called.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectTag, Tag, TagState } from '../../types';

// ── Fixtures ──

function project(id = 'p1'): Project {
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

function tag(id: string, groupId: string | null, position: number): Tag {
  return {
    id,
    groupId,
    name: id,
    color: 'gray',
    icon: null,
    position,
    createdAt: 0,
    updatedAt: 0,
  };
}

/** Status (icon group) and Priority, plus ungrouped q4 and infra. */
function catalog(assignments: ProjectTag[] = []): TagState {
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
      {
        id: 'priority',
        name: 'Priority',
        position: 1,
        isIconGroup: false,
        showInSidebar: false,
        createdAt: 0,
        updatedAt: 0,
      },
    ],
    tags: [
      tag('idea', 'status', 0),
      tag('done', 'status', 1),
      tag('blocked', 'status', 2),
      tag('p0', 'priority', 0),
      tag('q4', null, 0),
      tag('infra', null, 1),
    ],
    assignments,
  };
}

function deferredWrite() {
  let resolve!: () => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

// ── Mock plumbing ──

/** What the backend holds; every `get_tag_state` answers with it. */
let server: TagState;
let setProjectGroupTag: ReturnType<typeof vi.fn>;
let setProjectTagAssigned: ReturnType<typeof vi.fn>;
let createTag: ReturnType<typeof vi.fn>;
let deleteTag: ReturnType<typeof vi.fn>;
let moveTag: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;
let deleting: Set<string>;

/** Apply a group write to `server`, as the backend would. */
function commitGroupTag(projectId: string, groupId: string, tagId: string | null): void {
  const inGroup = new Set(server.tags.filter((t) => t.groupId === groupId).map((t) => t.id));
  server = {
    ...server,
    assignments: [
      ...server.assignments.filter((a) => a.projectId !== projectId || !inGroup.has(a.tagId)),
      ...(tagId ? [{ projectId, tagId }] : []),
    ],
  };
}

function commitTagAssigned(projectId: string, tagId: string, assigned: boolean): void {
  const rest = server.assignments.filter((a) => a.projectId !== projectId || a.tagId !== tagId);
  server = { ...server, assignments: assigned ? [...rest, { projectId, tagId }] : rest };
}

async function setup(initial: TagState = catalog()) {
  server = initial;
  const { tagsStore } = await import('../../stores/tags.svelte');
  const { tagActions } = await import('./tagActions.svelte');
  await tagsStore.load();
  const statusOf = (projectId = 'p1') => tagsStore.iconTagFor(projectId)?.id ?? null;
  return { tagsStore, tagActions, statusOf };
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('window', new EventTarget());
  deleting = new Set();
  toastError = vi.fn();
  setProjectGroupTag = vi.fn(async (projectId: string, groupId: string, tagId: string | null) =>
    commitGroupTag(projectId, groupId, tagId)
  );
  setProjectTagAssigned = vi.fn(async (projectId: string, tagId: string, assigned: boolean) =>
    commitTagAssigned(projectId, tagId, assigned)
  );
  createTag = vi.fn();
  deleteTag = vi.fn(async (tagId: string) => {
    server = { ...server, tags: server.tags.filter((t) => t.id !== tagId) };
  });
  moveTag = vi.fn(async () => {});

  const commandsMock = {
    getTagState: vi.fn(async () => ({ data: structuredClone(server), revalidating: null })),
    setProjectGroupTag,
    setProjectTagAssigned,
    createTag,
    deleteTag,
    moveTag,
  };
  vi.doMock('../../commands', () => commandsMock);
  vi.doMock('../../api/commands', () => commandsMock);
  vi.doMock('../../transport', () => ({ listenToEvent: () => () => {} }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: {
      liveProjects: [],
      isProjectDeleting: (projectId: string) => deleting.has(projectId),
    },
  }));
  vi.doMock('svelte-sonner', () => ({ toast: { error: toastError } }));
});

afterEach(() => {
  vi.doUnmock('../../commands');
  vi.doUnmock('../../api/commands');
  vi.doUnmock('../../transport');
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('svelte-sonner');
  vi.unstubAllGlobals();
});

// ── Tests ──

describe('setProjectGroupTag', () => {
  it('shows the pick at once, then writes it', async () => {
    const write = deferredWrite();
    setProjectGroupTag.mockReturnValueOnce(write.promise);
    const { tagActions, statusOf } = await setup();

    const pick = tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    expect(statusOf()).toBe('blocked');
    await tick();
    expect(setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', 'blocked');

    commitGroupTag('p1', 'status', 'blocked');
    write.resolve();
    await pick;
    await tick();
    expect(statusOf()).toBe('blocked');
  });

  it('clears the group’s tag when going back to Default', async () => {
    const { tagActions, statusOf } = await setup(catalog([{ projectId: 'p1', tagId: 'done' }]));
    await tagActions.setProjectGroupTag(project(), 'status', null);
    expect(setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', null);
    await tick();
    expect(statusOf()).toBeNull();
  });

  it('no-ops when the project already shows that value', async () => {
    const { tagActions } = await setup(catalog([{ projectId: 'p1', tagId: 'done' }]));
    await tagActions.setProjectGroupTag(project(), 'status', 'done');
    await tagActions.setProjectGroupTag(project(), 'priority', null);
    expect(setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('ignores projects that are being deleted', async () => {
    const { tagActions, statusOf } = await setup();
    deleting.add('p1');
    await tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    expect(setProjectGroupTag).not.toHaveBeenCalled();
    expect(statusOf()).toBeNull();
  });

  it('surfaces a failed write and returns to server truth', async () => {
    setProjectGroupTag.mockRejectedValueOnce(new Error('db locked'));
    const { tagActions, statusOf } = await setup(catalog([{ projectId: 'p1', tagId: 'done' }]));
    await tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    expect(statusOf()).toBe('done');
    expect(toastError).toHaveBeenCalledWith('Unable to update tags', { description: 'db locked' });
  });

  it('persists rapid picks in order while immediately showing the latest', async () => {
    const writes = [deferredWrite(), deferredWrite(), deferredWrite()];
    const queue = [...writes];
    setProjectGroupTag.mockImplementation(
      async (projectId: string, groupId: string, tagId: string | null) => {
        await queue.shift()!.promise;
        commitGroupTag(projectId, groupId, tagId);
      }
    );
    const { tagActions, statusOf } = await setup();

    const first = tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    const second = tagActions.setProjectGroupTag(project(), 'status', 'done');
    const third = tagActions.setProjectGroupTag(project(), 'status', null);
    expect(statusOf()).toBeNull();
    await tick();
    expect(setProjectGroupTag.mock.calls).toEqual([['p1', 'status', 'blocked']]);

    writes[0].resolve();
    await first;
    await tick();
    expect(setProjectGroupTag.mock.calls.map((c) => c[2])).toEqual(['blocked', 'done']);
    // The older write's refetch can't show through the newer picks.
    expect(statusOf()).toBeNull();

    writes[1].resolve();
    await second;
    await tick();
    expect(setProjectGroupTag.mock.calls.map((c) => c[2])).toEqual(['blocked', 'done', null]);
    expect(statusOf()).toBeNull();

    writes[2].resolve();
    await third;
    await tick();
    expect(statusOf()).toBeNull();

    // Once drained, a later pick starts a fresh queue.
    setProjectGroupTag.mockImplementation(async (p: string, g: string, t: string | null) =>
      commitGroupTag(p, g, t)
    );
    await tagActions.setProjectGroupTag(project(), 'status', 'idea');
    expect(setProjectGroupTag).toHaveBeenLastCalledWith('p1', 'status', 'idea');
  });

  it('keeps a newer queued pick when an older write fails', async () => {
    const older = deferredWrite();
    setProjectGroupTag
      .mockReturnValueOnce(older.promise)
      .mockImplementationOnce(async (p: string, g: string, t: string | null) =>
        commitGroupTag(p, g, t)
      );
    const { tagActions, statusOf } = await setup();

    const first = tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    const second = tagActions.setProjectGroupTag(project(), 'status', 'done');
    older.reject(new Error('older write failed'));
    await first;
    expect(statusOf()).toBe('done');
    expect(toastError).toHaveBeenCalledWith('Unable to update tags', {
      description: 'older write failed',
    });

    await second;
    await tick();
    expect(setProjectGroupTag.mock.calls.map((c) => c[2])).toEqual(['blocked', 'done']);
    expect(statusOf()).toBe('done');
  });

  it('falls back to what the older write committed when the latest fails', async () => {
    setProjectGroupTag
      .mockImplementationOnce(async (p: string, g: string, t: string | null) =>
        commitGroupTag(p, g, t)
      )
      .mockRejectedValueOnce(new Error('latest write failed'));
    const { tagActions, statusOf } = await setup();

    const first = tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    const second = tagActions.setProjectGroupTag(project(), 'status', 'done');
    await Promise.all([first, second]);
    await tick();
    expect(statusOf()).toBe('blocked');
  });

  it('compares a pick with the pending one even if a refetch landed meanwhile', async () => {
    const write = deferredWrite();
    setProjectGroupTag.mockReturnValueOnce(write.promise);
    const { tagsStore, tagActions } = await setup();

    const first = tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    await tagsStore.requestRefresh();
    // Same as the pending pick: nothing more to write.
    await tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    const second = tagActions.setProjectGroupTag(project(), 'status', null);

    write.resolve();
    await Promise.all([first, second]);
    expect(setProjectGroupTag.mock.calls.map((c) => c[2])).toEqual(['blocked', null]);
  });

  it('runs writes to different groups and projects concurrently', async () => {
    const held = [deferredWrite(), deferredWrite(), deferredWrite()];
    setProjectGroupTag
      .mockReturnValueOnce(held[0].promise)
      .mockReturnValueOnce(held[1].promise)
      .mockReturnValueOnce(held[2].promise);
    const { tagActions, tagsStore, statusOf } = await setup();

    const writes = [
      tagActions.setProjectGroupTag(project(), 'status', 'blocked'),
      tagActions.setProjectGroupTag(project(), 'priority', 'p0'),
      tagActions.setProjectGroupTag(project('p2'), 'status', 'done'),
    ];
    await tick();
    expect(setProjectGroupTag.mock.calls).toEqual([
      ['p1', 'status', 'blocked'],
      ['p1', 'priority', 'p0'],
      ['p2', 'status', 'done'],
    ]);

    held[0].reject(new Error('status write failed'));
    await writes[0];
    expect(statusOf()).toBeNull();
    expect(tagsStore.groupTagFor('p1', 'priority')?.id).toBe('p0');
    expect(statusOf('p2')).toBe('done');
    held[1].resolve();
    held[2].resolve();
    await Promise.all(writes);
  });
});

describe('setProjectTagAssigned', () => {
  it('adds and removes an ungrouped tag', async () => {
    const { tagsStore, tagActions } = await setup(catalog([{ projectId: 'p1', tagId: 'infra' }]));
    const add = tagActions.setProjectTagAssigned(project(), 'q4', true);
    const remove = tagActions.setProjectTagAssigned(project(), 'infra', false);
    expect([...tagsStore.tagIdsFor('p1')]).toEqual(['q4']);
    await Promise.all([add, remove]);
    expect(setProjectTagAssigned.mock.calls).toEqual([
      ['p1', 'q4', true],
      ['p1', 'infra', false],
    ]);
  });

  it('no-ops when the tag is already in that state', async () => {
    const { tagActions } = await setup(catalog([{ projectId: 'p1', tagId: 'infra' }]));
    await tagActions.setProjectTagAssigned(project(), 'infra', true);
    await tagActions.setProjectTagAssigned(project(), 'q4', false);
    expect(setProjectTagAssigned).not.toHaveBeenCalled();
  });

  it('routes a grouped tag through its group’s slot', async () => {
    const { tagActions, statusOf } = await setup(catalog([{ projectId: 'p1', tagId: 'done' }]));
    await tagActions.setProjectTagAssigned(project(), 'idea', true);
    expect(setProjectGroupTag).toHaveBeenLastCalledWith('p1', 'status', 'idea');
    // Removing a tag the project doesn't carry is a no-op...
    await tagActions.setProjectTagAssigned(project(), 'blocked', false);
    expect(setProjectGroupTag).toHaveBeenCalledTimes(1);
    // ...removing the one it does clears the group.
    await tagActions.setProjectTagAssigned(project(), 'idea', false);
    expect(setProjectGroupTag).toHaveBeenLastCalledWith('p1', 'status', null);
    expect(setProjectTagAssigned).not.toHaveBeenCalled();
    await tick();
    expect(statusOf()).toBeNull();
  });

  it('surfaces a failure and returns to server truth', async () => {
    setProjectTagAssigned.mockRejectedValueOnce(new Error('nope'));
    const { tagsStore, tagActions } = await setup();
    await tagActions.setProjectTagAssigned(project(), 'q4', true);
    expect(tagsStore.tagIdsFor('p1').size).toBe(0);
    expect(toastError).toHaveBeenCalledWith('Unable to update tags', { description: 'nope' });
  });
});

describe('createTagForProject', () => {
  it('creates and assigns in one write, showing the new tag at once', async () => {
    const shipping = tag('shipping', 'status', 3);
    createTag.mockImplementation(async () => {
      server = { ...server, tags: [...server.tags, shipping] };
      commitGroupTag('p1', 'status', 'shipping');
      return shipping;
    });
    const { tagActions, statusOf } = await setup(catalog([{ projectId: 'p1', tagId: 'done' }]));

    const created = await tagActions.createTagForProject(project(), 'shipping', 'status');
    expect(created).toEqual(shipping);
    expect(createTag).toHaveBeenCalledWith({
      name: 'shipping',
      groupId: 'status',
      assignProjectId: 'p1',
    });
    expect(statusOf()).toBe('shipping');
    await tick();
    expect(statusOf()).toBe('shipping');
  });

  it('queues a grouped create behind earlier picks in the group', async () => {
    const pick = deferredWrite();
    setProjectGroupTag.mockReturnValueOnce(pick.promise);
    const shipping = tag('shipping', 'status', 3);
    createTag.mockImplementation(async () => {
      server = { ...server, tags: [...server.tags, shipping] };
      commitGroupTag('p1', 'status', 'shipping');
      return shipping;
    });
    const { tagActions, statusOf } = await setup();

    const picking = tagActions.setProjectGroupTag(project(), 'status', 'blocked');
    const creating = tagActions.createTagForProject(project(), 'shipping', 'status');
    await tick();
    expect(createTag).not.toHaveBeenCalled();
    pick.resolve();
    await Promise.all([picking, creating]);
    expect(createTag).toHaveBeenCalledTimes(1);
    expect(statusOf()).toBe('shipping');
  });

  it('toasts and resolves null when the create fails', async () => {
    createTag.mockRejectedValueOnce(new Error('A tag named “q4” already exists'));
    const { tagActions } = await setup();
    expect(await tagActions.createTagForProject(project(), 'Q4')).toBeNull();
    expect(toastError).toHaveBeenCalledWith('Unable to create tag', {
      description: 'A tag named “q4” already exists',
    });
  });
});

describe('catalog writes', () => {
  it('keeps the chain going when a patch throws, and refetches instead', async () => {
    const shipping = tag('shipping', null, 2);
    createTag.mockImplementation(async () => {
      server = { ...server, tags: [...server.tags, shipping] };
      return shipping;
    });
    const { tagsStore, tagActions } = await setup();
    vi.spyOn(tagsStore, 'patchLocal').mockImplementationOnce(() => {
      throw new Error('bad patch');
    });
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {});

    // The write landed, so the create still resolves to its tag...
    expect(await tagActions.createTag({ name: 'shipping', groupId: null })).toEqual(shipping);
    expect(logged).toHaveBeenCalledTimes(1);
    expect(toastError).not.toHaveBeenCalled();
    // ...and the refetch, not the patch, shows it.
    await tick();
    expect(tagsStore.tagsById.has('shipping')).toBe(true);

    // The catalog chain is still alive: the next write runs.
    expect(await tagActions.deleteTag(shipping)).toBe(true);
    expect(deleteTag).toHaveBeenCalledWith('shipping');
    await tick();
    expect(tagsStore.tagsById.has('shipping')).toBe(false);
    logged.mockRestore();
  });

  it('skips a move its caller no longer wants, without a toast or a write', async () => {
    const { tagsStore, tagActions } = await setup();
    const move = { tagId: 'idea', groupId: 'status', index: 2 };
    expect(await tagActions.moveTag(move, { stillWanted: () => false })).toBe(false);
    expect(moveTag).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
    expect(tagsStore.tagsById.get('idea')?.position).toBe(0);

    // The chain carries on past it.
    expect(await tagActions.moveTag(move)).toBe(true);
    expect(moveTag).toHaveBeenCalledWith('idea', 'status', 2);
  });

  it('settles one write with its caller before asking whether the next is wanted', async () => {
    moveTag.mockRejectedValueOnce(new Error('db locked'));
    const { tagActions } = await setup();
    const order: string[] = [];
    const first = tagActions.moveTag(
      { tagId: 'idea', groupId: 'status', index: 2 },
      { onSettled: (ok) => order.push(`first settled ${ok}`) }
    );
    const second = tagActions.moveTag(
      { tagId: 'done', groupId: 'status', index: 0 },
      {
        stillWanted: () => {
          order.push('second asked');
          return false;
        },
        onSettled: (ok) => order.push(`second settled ${ok}`),
      }
    );
    expect(await Promise.all([first, second])).toEqual([false, false]);
    expect(order).toEqual(['first settled false', 'second asked', 'second settled false']);
    expect(moveTag).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledTimes(1);
  });
});
