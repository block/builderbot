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

  it('settles on timeout when the store read hangs after init', async () => {
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

  it('waits out a slow store init and still restores the route', async () => {
    // The realistic degradation: preferences_store_path + load() are slow
    // (cold disk, remote web), but the key read is in-memory once init lands.
    // `<main>` is gated on preferences.loaded, which waits on the same init,
    // so the timeout must not start until init resolves.
    vi.useFakeTimers();
    let resolveInit!: () => void;
    initPersistentStore.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        resolveInit = resolve;
      })
    );
    getStoreValue.mockResolvedValueOnce('p1');
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { restoreInitialRoute, navigation, INITIAL_ROUTE_TIMEOUT_MS } = await importNavigation();

    const restore = restoreInitialRoute();
    await vi.advanceTimersByTimeAsync(INITIAL_ROUTE_TIMEOUT_MS * 3);
    // Init is still pending: nothing read, nothing settled, no timeout fired.
    expect(getStoreValue).not.toHaveBeenCalled();
    expect(navigation.initialRouteSettled).toBe(false);
    expect(warn).not.toHaveBeenCalled();

    resolveInit();
    await restore;

    expect(getStoreValue).toHaveBeenCalledWith('last-viewed-project');
    expect(navigation.detailStack).toEqual([
      { kind: 'projects' },
      { kind: 'project', projectId: 'p1' },
    ]);
    expect(navigation.initialRouteSettled).toBe(true);
    expect(warn).not.toHaveBeenCalled();
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
    // The grid is this window's base route until a seed lands, so it must be
    // retained here or a window with no seed renders two empty layers.
    expect(navigation.homeLayerRetained).toBe(true);
  });

  // A secondary window settles at the root before initNavigation reads its
  // seed. If the seed never lands the grid is all the window has, so the
  // settle must have retained it: there is no Back affordance at the root to
  // trigger a goHome that would.
  it.each([
    ['rejects', () => takeWindowSeed.mockRejectedValueOnce(new Error('no seed'))],
    ['is null', () => takeWindowSeed.mockResolvedValueOnce(null)],
    [
      'names a deleted project',
      () => {
        takeWindowSeed.mockResolvedValueOnce('gone');
        projectsStoreMock.loaded = true;
        projectsStoreMock.projects = [{ id: 'p1' }];
      },
    ],
  ])('keeps the grid for a secondary window whose seed %s', async (_, arrange) => {
    windowLabel = 'win-2';
    arrange();
    const { restoreInitialRoute, initNavigation, navigation } = await importNavigation();

    await restoreInitialRoute();
    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.initialRouteSettled).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
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

describe('workspace layers', () => {
  // The workspace is two stacked layers (App.svelte): the home grid, mounted
  // once `homeLayerRetained` and active while `homeActive`, and the project
  // layer, holding the last project / repos base route in `projectLayer`.

  it('seeds a cold Tauri start at the root with nothing retained until the route settles', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, navigation } = await importNavigation();

    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(false);
    expect(navigation.projectLayer).toBeNull();

    await restoreInitialRoute();

    // Restored into a project: the grid was never shown, so it isn't retained.
    expect(navigation.homeActive).toBe(false);
    expect(navigation.homeLayerRetained).toBe(false);
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });
  });

  it('retains home once a start without a persisted project settles at the root', async () => {
    getStoreValue.mockResolvedValueOnce(null);
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
    expect(navigation.projectLayer).toBeNull();
  });

  it('seeds the project layer from the web snapshot without retaining home', async () => {
    snapshotProjectId = 'p1';
    windowLabel = null;
    const { restoreInitialRoute, navigation } = await importNavigation();

    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });

    await restoreInitialRoute();
    expect(navigation.homeLayerRetained).toBe(false);
  });

  it('goHome keeps the project layer and activates and retains home', async () => {
    const { restoreInitialRoute, selectProject, goHome, navigation } = await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');
    const layer = navigation.projectLayer;

    goHome();

    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    expect(navigation.selectedProjectId).toBeNull();
  });

  it('keeps the layer object identity while the project is unchanged and replaces it otherwise', async () => {
    const { restoreInitialRoute, selectProject, goHome, showAllRepos, navigation } =
      await importNavigation();
    await restoreInitialRoute();

    selectProject('p1');
    const p1 = navigation.projectLayer;
    expect(p1).toEqual({ kind: 'project', projectId: 'p1' });
    goHome();
    selectProject('p1');
    expect(navigation.projectLayer).toBe(p1);

    goHome();
    selectProject('p2');
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p2' });
    expect(navigation.homeActive).toBe(false);

    showAllRepos();
    const repos = navigation.projectLayer;
    expect(repos).toEqual({ kind: 'repos' });
    goHome();
    showAllRepos();
    expect(navigation.projectLayer).toBe(repos);
  });

  it('keeps both layers through settings and a diff over home', async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const {
      restoreInitialRoute,
      selectProject,
      goHome,
      openSettings,
      openDiffRoute,
      popDetailRoute,
      navigation,
    } = await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');
    goHome();
    const layer = navigation.projectLayer;

    openSettings();
    expect(navigation.workspaceParked).toBe(true);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    popDetailRoute();

    // A diff that names a project no longer mounts that project's view under
    // it: the layers follow the base route, which is still home.
    openDiffRoute({ branchId: 'b1' });
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    openDiffRoute({ branchId: 'b2', projectId: 'p2' });
    expect(navigation.currentRoute.kind).toBe('diff');
    expect(navigation.selectedProjectId).toBe('p2');
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    popDetailRoute();
    expect(navigation.homeActive).toBe(true);
  });

  it('keeps home inactive and unparks back to the project under settings and a diff', async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const {
      restoreInitialRoute,
      selectProject,
      openSettings,
      openDiffRoute,
      popDetailRoute,
      navigation,
    } = await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');
    const layer = navigation.projectLayer;

    openDiffRoute({ branchId: 'b1', projectId: 'p1' });
    openSettings();
    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toBe(layer);
    popDetailRoute();
    popDetailRoute();
    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toBe(layer);
  });

  it('releases the project layer only on home with a matching id', async () => {
    const { restoreInitialRoute, selectProject, goHome, releaseProjectLayer, navigation } =
      await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');

    // On the project itself, the layer is the view on screen.
    releaseProjectLayer('p1');
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });

    goHome();
    releaseProjectLayer('p2');
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });
    releaseProjectLayer('p1');
    expect(navigation.projectLayer).toBeNull();

    // Nothing to release: a no-op rather than a throw.
    releaseProjectLayer('p1');
    expect(navigation.projectLayer).toBeNull();
  });
});

describe('followWhileVisible', () => {
  // Under vitest $state.raw is a plain variable. $effect.pre runs once when it
  // is set up, like at mount, and `flush` reruns it after a change, after its
  // cleanup, the way Svelte would.
  let effects: Array<{ run: () => void | (() => void); cleanup?: void | (() => void) }>;
  let layerVisible: boolean;

  beforeEach(() => {
    vi.useFakeTimers();
    effects = [];
    layerVisible = true;
    const identity = (value: unknown) => value;
    const track = (run: () => void | (() => void)) => effects.push({ run, cleanup: run() });
    vi.stubGlobal('$state', Object.assign(identity, { raw: identity }));
    vi.stubGlobal('$effect', Object.assign(track, { pre: track }));
    vi.doMock('./workspaceLayer', () => ({
      useWorkspaceLayer: () => ({
        get visible() {
          return layerVisible;
        },
      }),
    }));
  });

  afterEach(() => {
    vi.doUnmock('./workspaceLayer');
  });

  function flush() {
    for (const effect of effects) {
      effect.cleanup?.();
      effect.cleanup = effect.run();
    }
  }

  it('follows the source in the same flush while the layer is visible', async () => {
    const { followWhileVisible } = await importNavigation();
    let source = 'a';
    const followed = followWhileVisible(() => source);
    expect(followed.current).toBe('a');

    source = 'b';
    flush();
    expect(followed.current).toBe('b');
    expect(vi.getTimerCount()).toBe(0);
  });

  it('follows the source in a hidden layer once it stops changing', async () => {
    const { followWhileVisible, HIDDEN_FOLLOW_DELAY_MS } = await importNavigation();
    let source = '';
    const followed = followWhileVisible(() => source);
    layerVisible = false;
    flush();

    source = 's';
    flush();
    await vi.advanceTimersByTimeAsync(HIDDEN_FOLLOW_DELAY_MS - 1);
    source = 'sl';
    flush();
    // The second keystroke restarted the wait.
    await vi.advanceTimersByTimeAsync(HIDDEN_FOLLOW_DELAY_MS - 1);
    expect(followed.current).toBe('');

    await vi.advanceTimersByTimeAsync(1);
    expect(followed.current).toBe('sl');
  });

  it('catches up in the flush that shows the layer without waiting', async () => {
    const { followWhileVisible } = await importNavigation();
    let source = 'slow';
    const followed = followWhileVisible(() => source);
    layerVisible = false;
    flush();

    source = '';
    flush();
    layerVisible = true;
    flush();

    expect(followed.current).toBe('');
    // The hidden wait was cancelled rather than left to fire later.
    expect(vi.getTimerCount()).toBe(0);
  });

  it('seeds from the source when set up in a hidden layer', async () => {
    layerVisible = false;
    const { followWhileVisible } = await importNavigation();
    const followed = followWhileVisible(() => 'seed');

    expect(followed.current).toBe('seed');
  });
});
