import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import { withSortKeyAdded, withoutSortKey, type ProjectSort } from './projectSort';

// Status (icon group: idea, doing, done), Priority (p0, p1), ungrouped q4, infra.
const catalog = indexTagCatalog(tagState());

/** What preferences.json holds, by key. */
let stored: Map<string, unknown>;
let getStoreValue: ReturnType<typeof vi.fn>;
let setStoreValue: ReturnType<typeof vi.fn>;

async function importPrefs() {
  return await import('./projectSortPrefs.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (agent.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  stored = new Map();
  getStoreValue = vi.fn(async (key: string) => stored.get(key));
  setStoreValue = vi.fn(async (key: string, value: unknown) => {
    stored.set(key, structuredClone(value));
  });
  vi.doMock('../../shared/persistentStore', () => ({ getStoreValue, setStoreValue }));
});

afterEach(() => {
  vi.doUnmock('../../shared/persistentStore');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('loadProjectSortPrefs', () => {
  it('starts with no sort, and stays there when nothing is stored', async () => {
    const { projectSortPrefs, loadProjectSortPrefs } = await importPrefs();
    expect(projectSortPrefs.sort).toEqual([]);

    await loadProjectSortPrefs();

    expect(getStoreValue).toHaveBeenCalledWith('projects-sort');
    expect(projectSortPrefs.sort).toEqual([]);
    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('reads the stored sort, in order', async () => {
    stored.set('projects-sort', [
      { kind: 'group', groupId: 'priority', direction: 'asc' },
      { kind: 'updated', direction: 'desc' },
    ]);
    const { projectSortPrefs, loadProjectSortPrefs } = await importPrefs();

    await loadProjectSortPrefs();

    expect(projectSortPrefs.sort).toEqual([
      { kind: 'group', groupId: 'priority', direction: 'asc' },
      { kind: 'updated', direction: 'desc' },
    ]);
  });

  it('reads a stored value tolerantly: malformed entries, unknown kinds, repeats', async () => {
    stored.set('projects-sort', [
      { kind: 'created', direction: 'desc', extra: true },
      'updated',
      null,
      { kind: 'name', direction: 'asc' },
      { kind: 'updated', direction: 'sideways' },
      { kind: 'group', direction: 'asc' },
      { kind: 'created', direction: 'asc' },
      { kind: 'group', groupId: 'status', direction: 'desc' },
    ]);
    const { projectSortPrefs, loadProjectSortPrefs } = await importPrefs();

    await loadProjectSortPrefs();

    expect(projectSortPrefs.sort).toEqual([
      { kind: 'created', direction: 'desc' },
      { kind: 'group', groupId: 'status', direction: 'desc' },
    ]);
  });

  it.each([['a string'], [42], [{ kind: 'created', direction: 'desc' }], [null]])(
    'reads %j as no sort',
    async (value) => {
      stored.set('projects-sort', value);
      const { projectSortPrefs, loadProjectSortPrefs } = await importPrefs();

      await loadProjectSortPrefs();

      expect(projectSortPrefs.sort).toEqual([]);
    }
  );

  it('keeps a key for a group the catalog no longer has, and writes nothing', async () => {
    // Held as stored: readers resolve it, and the next edit prunes it.
    stored.set('projects-sort', [{ kind: 'group', groupId: 'gone', direction: 'asc' }]);
    const { projectSortPrefs, loadProjectSortPrefs } = await importPrefs();

    await loadProjectSortPrefs();

    expect(projectSortPrefs.sort).toEqual([{ kind: 'group', groupId: 'gone', direction: 'asc' }]);
    expect(setStoreValue).not.toHaveBeenCalled();
  });
});

describe('setProjectSort', () => {
  it('sorts the window and saves the sort under projects-sort', async () => {
    const { projectSortPrefs, setProjectSort } = await importPrefs();
    const sort: ProjectSort = [
      { kind: 'updated', direction: 'desc' },
      { kind: 'group', groupId: 'priority', direction: 'asc' },
    ];

    setProjectSort(sort);

    expect(projectSortPrefs.sort).toEqual(sort);
    expect(setStoreValue).toHaveBeenCalledTimes(1);
    expect(setStoreValue).toHaveBeenCalledWith('projects-sort', sort);
  });

  it('writes nothing for the sort the window already has', async () => {
    const { projectSortPrefs, setProjectSort } = await importPrefs();
    setProjectSort([{ kind: 'created', direction: 'asc' }]);
    setStoreValue.mockClear();

    // The array itself, as an edit helper returns when it changes nothing…
    setProjectSort(projectSortPrefs.sort);
    // …and an equal one.
    setProjectSort([{ kind: 'created', direction: 'asc' }]);

    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('writes nothing when an empty sort is reset', async () => {
    const { setProjectSort } = await importPrefs();

    setProjectSort([]);

    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('saves a flipped direction and a new order, which are different sorts', async () => {
    const { projectSortPrefs, setProjectSort } = await importPrefs();
    setProjectSort([
      { kind: 'created', direction: 'asc' },
      { kind: 'updated', direction: 'desc' },
    ]);

    setProjectSort([
      { kind: 'created', direction: 'desc' },
      { kind: 'updated', direction: 'desc' },
    ]);
    setProjectSort([
      { kind: 'updated', direction: 'desc' },
      { kind: 'created', direction: 'desc' },
    ]);

    expect(setStoreValue).toHaveBeenCalledTimes(3);
    expect(projectSortPrefs.sort).toEqual([
      { kind: 'updated', direction: 'desc' },
      { kind: 'created', direction: 'desc' },
    ]);
  });

  it('saves a clean copy: no repeats, no stray fields', async () => {
    const { projectSortPrefs, setProjectSort } = await importPrefs();

    setProjectSort([
      { kind: 'updated', direction: 'desc', note: 'x' },
      { kind: 'updated', direction: 'asc' },
    ] as unknown as ProjectSort);

    expect(projectSortPrefs.sort).toEqual([{ kind: 'updated', direction: 'desc' }]);
    expect(setStoreValue).toHaveBeenCalledWith('projects-sort', [
      { kind: 'updated', direction: 'desc' },
    ]);
  });

  it('prunes a deleted group’s key on the next edit, through the edit helpers', async () => {
    stored.set('projects-sort', [
      { kind: 'group', groupId: 'gone', direction: 'asc' },
      { kind: 'created', direction: 'desc' },
    ]);
    const { projectSortPrefs, loadProjectSortPrefs, setProjectSort } = await importPrefs();
    await loadProjectSortPrefs();

    setProjectSort(
      withSortKeyAdded(
        projectSortPrefs.sort,
        { kind: 'group', groupId: 'priority', direction: 'asc' },
        catalog
      )
    );

    expect(stored.get('projects-sort')).toEqual([
      { kind: 'created', direction: 'desc' },
      { kind: 'group', groupId: 'priority', direction: 'asc' },
    ]);
  });

  it('saves the empty sort when the last key is removed', async () => {
    const { projectSortPrefs, setProjectSort } = await importPrefs();
    setProjectSort([{ kind: 'created', direction: 'asc' }]);

    setProjectSort(withoutSortKey(projectSortPrefs.sort, 'created', catalog));

    expect(projectSortPrefs.sort).toEqual([]);
    expect(setStoreValue).toHaveBeenLastCalledWith('projects-sort', []);
  });

  it('keeps the sort for this window when the save fails', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    setStoreValue.mockRejectedValueOnce(new Error('disk full'));
    const { projectSortPrefs, setProjectSort } = await importPrefs();

    setProjectSort([{ kind: 'created', direction: 'asc' }]);
    await Promise.resolve();
    await Promise.resolve();

    expect(projectSortPrefs.sort).toEqual([{ kind: 'created', direction: 'asc' }]);
    expect(error).toHaveBeenCalled();
  });

  it('does not follow other windows: nothing listens to the store', async () => {
    // The mocked persistentStore has no change listeners to offer; importing
    // and using the module must not reach for one.
    const { loadProjectSortPrefs, setProjectSort } = await importPrefs();
    await loadProjectSortPrefs();
    setProjectSort([{ kind: 'updated', direction: 'desc' }]);
    expect(getStoreValue).toHaveBeenCalledTimes(1);
  });
});
