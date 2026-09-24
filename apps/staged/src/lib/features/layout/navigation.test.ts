import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// ── Mock plumbing ──

let windowLabel: string | null;
let getStoreValue: ReturnType<typeof vi.fn>;
let setStoreValue: ReturnType<typeof vi.fn>;
let writeSnapshot: ReturnType<typeof vi.fn>;
let clearSnapshot: ReturnType<typeof vi.fn>;
let snapshotProjectId: string | null;
let projectsStoreMock: {
  projects: Array<{ id: string }>;
  loaded: boolean;
  ensureLoaded: ReturnType<typeof vi.fn>;
  whenLoaded: ReturnType<typeof vi.fn>;
};
let takeWindowSeed: ReturnType<typeof vi.fn>;
let pruneDeletedProjects: ReturnType<typeof vi.fn>;

async function importNavigation() {
  return await import('./navigation.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (projectsData.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);

  windowLabel = 'main';
  getStoreValue = vi.fn().mockResolvedValue(null);
  setStoreValue = vi.fn().mockResolvedValue(undefined);
  writeSnapshot = vi.fn();
  clearSnapshot = vi.fn();
  snapshotProjectId = null;
  projectsStoreMock = {
    projects: [],
    loaded: false,
    ensureLoaded: vi.fn().mockResolvedValue(undefined),
    whenLoaded: vi.fn().mockResolvedValue(undefined),
  };
  takeWindowSeed = vi.fn().mockResolvedValue(null);
  pruneDeletedProjects = vi.fn().mockResolvedValue(undefined);

  vi.doMock('../../transport', () => ({
    getWindowLabel: () => windowLabel,
  }));
  vi.doMock('../../shared/persistentStore', () => ({
    getStoreValue,
    setStoreValue,
  }));
  vi.doMock('../../shared/webSnapshot', () => ({
    SNAPSHOT_KEYS: { lastProject: 'staged:boot:last-project' },
    readSnapshot: () => snapshotProjectId,
    writeSnapshot,
    clearSnapshot,
  }));
  vi.doMock('../../commands', () => ({ takeWindowSeed }));
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: {
      isUnread: () => false,
      markAsRead: vi.fn(),
      pruneDeletedProjects,
    },
  }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: projectsStoreMock,
  }));
  vi.doMock('../projects/projectsListViewState.svelte', () => ({
    requestProjectsListRestore: vi.fn(),
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetAllMocks();
});

describe('initial project restore', () => {
  it('restores the synchronous snapshot in the first window and web mode', async () => {
    snapshotProjectId = 'persisted';
    windowLabel = 'main';
    let module = await importNavigation();
    expect(module.navigation.selectedProjectId).toBe('persisted');
    expect(module.navigation.canGoBack).toBe(true);
    expect(module.navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'persisted' },
    ]);
    expect(module.navigation.currentRoute).toEqual({ kind: 'project', projectId: 'persisted' });

    vi.resetModules();
    windowLabel = null;
    module = await importNavigation();
    expect(module.navigation.selectedProjectId).toBe('persisted');
    expect(module.navigation.canGoBack).toBe(true);
    expect(module.navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'persisted' },
    ]);
  });

  it('starts a secondary window empty instead of leaking the shared snapshot', async () => {
    snapshotProjectId = 'persisted';
    windowLabel = 'win-2';

    const { navigation } = await importNavigation();

    expect(navigation.selectedProjectId).toBeNull();
    expect(navigation.canGoBack).toBe(false);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
  });
});

describe('persistLastProject', () => {
  it('writes the legacy unsuffixed key in the first window', async () => {
    windowLabel = 'main';
    const { selectProject } = await importNavigation();

    selectProject('p1');

    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', 'p1');
    expect(writeSnapshot).toHaveBeenCalledWith('staged:boot:last-project', 'p1');
  });

  it('writes the legacy unsuffixed key in web mode (no window label)', async () => {
    windowLabel = null;
    const { selectProject } = await importNavigation();

    selectProject('p1');

    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', 'p1');
  });

  it('persists nothing in a secondary window', async () => {
    windowLabel = 'win-2';
    const { selectProject, goHome } = await importNavigation();

    // Navigating to a project and back home both persist in `main`; neither may
    // write a `…:win-2` key nobody ever reads.
    selectProject('p1');
    goHome();

    expect(setStoreValue).not.toHaveBeenCalled();
    expect(writeSnapshot).not.toHaveBeenCalled();
    expect(clearSnapshot).not.toHaveBeenCalled();
  });

  it('still navigates in a secondary window', async () => {
    windowLabel = 'win-2';
    const { selectProject, navigation } = await importNavigation();

    selectProject('p1');

    expect(navigation.selectedProjectId).toBe('p1');
  });
});

describe('initNavigation', () => {
  it('leaves a seeded web stack untouched when the project still exists', async () => {
    snapshotProjectId = 'p1';
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(projectsStoreMock.whenLoaded).toHaveBeenCalledTimes(1);
    expect(projectsStoreMock.ensureLoaded).not.toHaveBeenCalled();
    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p1' },
    ]);
    expect(navigation.canGoBack).toBe(true);
  });

  it('resets a seeded stack and clears persistence when the restored project is gone', async () => {
    snapshotProjectId = 'p1';
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.selectedProjectId).toBeNull();
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', null);
    expect(clearSnapshot).toHaveBeenCalledWith('staged:boot:last-project');
  });

  it('does not undo a goHome that happens before validation resolves', async () => {
    snapshotProjectId = 'p1';
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const pending = new Promise<void>((resolve) => setTimeout(resolve, 0));
    projectsStoreMock.whenLoaded.mockReturnValueOnce(pending);
    const { initNavigation, goHome, navigation } = await importNavigation();

    const restore = initNavigation();
    goHome();
    await restore;

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.selectedProjectId).toBeNull();
  });

  it('pushes the stored project on a pristine Tauri stack', async () => {
    snapshotProjectId = null;
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p1' },
    ]);
    expect(navigation.selectedProjectId).toBe('p1');
  });

  it('clears a missing stored Tauri project even from a pristine stack', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', null);
    expect(clearSnapshot).toHaveBeenCalledWith('staged:boot:last-project');
  });

  it('does not clobber user navigation while validating a stored Tauri project', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }, { id: 'p2' }];
    const pending = new Promise<void>((resolve) => setTimeout(resolve, 0));
    projectsStoreMock.whenLoaded.mockReturnValueOnce(pending);
    const { initNavigation, selectProject, navigation } = await importNavigation();

    const restore = initNavigation();
    selectProject('p2');
    await restore;

    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p2' },
    ]);
    expect(navigation.selectedProjectId).toBe('p2');
  });

  it('pushes a valid secondary-window seed after validation', async () => {
    windowLabel = 'win-2';
    takeWindowSeed.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p1' },
    ]);
    expect(navigation.selectedProjectId).toBe('p1');
  });
});
