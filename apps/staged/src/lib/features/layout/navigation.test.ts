import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// ── Mock plumbing ──

let windowLabel: string | null;
let getStoreValue: ReturnType<typeof vi.fn>;
let initPersistentStore: ReturnType<typeof vi.fn>;
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
  initPersistentStore = vi.fn().mockResolvedValue(undefined);
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
    initPersistentStore,
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
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.resetAllMocks();
});

describe('restoreInitialRoute', () => {
  it('pushes the stored project before settling on an untouched Tauri stack', async () => {
    windowLabel = 'main';
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, navigation } = await importNavigation();
    expect(navigation.initialRouteSettled).toBe(false);

    await restoreInitialRoute();

    expect(initPersistentStore).toHaveBeenCalledTimes(1);
    expect(getStoreValue).toHaveBeenCalledWith('last-viewed-project');
    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p1' },
    ]);
    expect(navigation.selectedProjectId).toBe('p1');
    expect(navigation.canGoBack).toBe(true);
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('settles at the root when the stored value is null', async () => {
    getStoreValue.mockResolvedValueOnce(null);
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.selectedProjectId).toBeNull();
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('settles on timeout when the store read hangs', async () => {
    vi.useFakeTimers();
    getStoreValue.mockReturnValueOnce(new Promise(() => {}));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { restoreInitialRoute, navigation, INITIAL_ROUTE_TIMEOUT_MS } = await importNavigation();

    const restore = restoreInitialRoute();
    await vi.advanceTimersByTimeAsync(INITIAL_ROUTE_TIMEOUT_MS - 1);
    expect(navigation.initialRouteSettled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await restore;

    expect(navigation.initialRouteSettled).toBe(true);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('settles when the store read rejects', async () => {
    getStoreValue.mockRejectedValueOnce(new Error('store down'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.initialRouteSettled).toBe(true);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('leaves a web-snapshot-seeded stack alone', async () => {
    snapshotProjectId = 'p1';
    windowLabel = null;
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(initPersistentStore).not.toHaveBeenCalled();
    expect(getStoreValue).not.toHaveBeenCalled();
    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p1' },
    ]);
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('leaves secondary windows unchanged', async () => {
    windowLabel = 'win-2';
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(initPersistentStore).not.toHaveBeenCalled();
    expect(getStoreValue).not.toHaveBeenCalled();
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('does not override navigation that happened while the store was read', async () => {
    let resolveRead!: (value: string) => void;
    getStoreValue.mockReturnValueOnce(
      new Promise<string>((resolve) => {
        resolveRead = resolve;
      })
    );
    const { restoreInitialRoute, selectProject, navigation } = await importNavigation();

    const restore = restoreInitialRoute();
    selectProject('p2');
    resolveRead('p1');
    await restore;

    expect(navigation.selectedProjectId).toBe('p2');
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('hands a restored project to initNavigation as a seeded stack', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { restoreInitialRoute, initNavigation, navigation } = await importNavigation();

    await restoreInitialRoute();
    await initNavigation();

    // Validation reused the pushed route rather than re-reading the store,
    // and backed out because the project is gone.
    expect(getStoreValue).toHaveBeenCalledTimes(1);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', null);
  });
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

describe('parked workspace', () => {
  it('keeps the repos list as the workspace under settings and restores it on Back', async () => {
    const { showAllRepos, openSettings, popDetailRoute, navigation } = await importNavigation();

    showAllRepos();
    expect(navigation.showReposList).toBe(true);
    expect(navigation.workspaceParked).toBe(false);

    openSettings('general');
    expect(navigation.activeView).toBe('settings');
    expect(navigation.workspaceParked).toBe(true);
    // The workspace's inputs come from the route underneath, so ReposListView
    // stays mounted rather than being swapped for ProjectsList under the cover.
    expect(navigation.showReposList).toBe(true);
    expect(navigation.selectedProjectId).toBeNull();

    popDetailRoute();
    expect(navigation.workspaceParked).toBe(false);
    expect(navigation.showReposList).toBe(true);
  });

  it('keeps the project under a diff and settings and parks for both', async () => {
    // Popping the diff route announces itself on `window` (node has none).
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const { selectProject, openDiffRoute, openSettings, popDetailRoute, navigation } =
      await importNavigation();

    selectProject('p1');
    openDiffRoute({ branchId: 'b1', projectId: 'p1' });
    expect(navigation.workspaceParked).toBe(true);
    expect(navigation.activeView).toBe('workspace');
    expect(navigation.selectedProjectId).toBe('p1');

    openSettings('doctor');
    expect(navigation.workspaceParked).toBe(true);
    expect(navigation.activeView).toBe('settings');
    expect(navigation.settingsSection).toBe('doctor');
    expect(navigation.selectedProjectId).toBe('p1');

    popDetailRoute();
    expect(navigation.currentRoute.kind).toBe('diff');
    expect(navigation.workspaceParked).toBe(true);

    popDetailRoute();
    expect(navigation.currentRoute).toEqual({ kind: 'project', projectId: 'p1' });
    expect(navigation.workspaceParked).toBe(false);
  });
});
